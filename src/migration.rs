use crate::global_config::GlobalConfig;
use crate::models::elector::PartialElectorData;
use crate::models::old_models::{OldMcStateExtra, OldShardDescription, OldShardStateUnsplit};
use anyhow::{Context, Result};
use memmap2::Mmap;
use serde::Serialize;
use std::collections::BTreeMap;
use std::fs::File;
use std::path::Path;
use std::sync::OnceLock;
use tycho_types::abi::{AbiType, AbiValue, AbiVersion, FromAbi, IntoAbi, WithAbiType};
use tycho_types::boc::Boc;
use tycho_types::cell::{CellBuilder, HashBytes, Lazy, Load};
use tycho_types::dict::AugDict;
use tycho_types::error::Error as TychoError;
use tycho_types::models as tycho;
use tycho_types::models::{
    AccountState, BlockchainConfig, BlockchainConfigParams, ConfigParam34, CurrencyCollection,
    DepthBalanceInfo, ShardAccount, ShardAccounts, ShardIdent, ShardStateUnsplit,
};
use tycho_types::num::Tokens;

#[derive(Debug, Copy, Clone, Serialize)]
pub struct ZerostateId {
    pub seqno: u32,
    pub root_hash: HashBytes,
    pub file_hash: HashBytes,
}
pub fn migrate_state(
    old_state: &OldShardStateUnsplit,
    config: Option<GlobalConfig>,
    shard_state_id: Option<ZerostateId>,
    shard_balance: Option<CurrencyCollection>,
    current_validator_set: Option<tycho::ValidatorSet>,
    time_ms: u64,
) -> Result<(ShardStateUnsplit, CurrencyCollection)> {
    let mut total_balance;
    let (accounts, custom) = {
        let mut shard_accounts = old_state
            .load_accounts()
            .context("failed to load accounts")?;

        total_balance = shard_accounts.root_extra().balance.clone();

        match old_state.load_custom()? {
            Some(custom) => {
                let Some(mut global_config) = config else {
                    anyhow::bail!("failed to load custom config");
                };

                // since validators are in masterchain update only if extra is present
                let stake = global_config.config.get_validator_stake_params()?;
                let validator_balance = add_validator_accounts(
                    &mut shard_accounts,
                    stake.min_stake,
                    global_config.validator_accounts,
                )?;
                total_balance = total_balance.checked_add(&validator_balance)?;

                println!("Updating masterchain config...");
                override_validator_set(
                    &mut global_config.config.params,
                    current_validator_set.as_ref(),
                )?;

                override_workchain_zerostates(&mut global_config.config.params, shard_state_id)?;
                println!("Mapped masterchain config");

                let config_address = global_config.config.address;
                if let Some((depth_balance, mut config_account)) =
                    shard_accounts.get(config_address)?
                {
                    update_config_account(&mut config_account, &global_config.config)?;
                    shard_accounts.set(config_address, depth_balance, config_account)?;
                    println!("Config contract was updated!");
                } else {
                    anyhow::bail!("failed to reset elector account");
                }

                // let elector_address = global_config.config.get_elector_address()?;
                // if let Some((depth_balance, mut elector_account)) =
                //     shard_accounts.get(elector_address)?
                // {
                //     reset_elector_account(&mut elector_account)?;
                //     shard_accounts.set(elector_address, depth_balance, elector_account)?;
                //     println!("Elector contract was reset!");
                // } else {
                //     anyhow::bail!("failed to reset elector account");
                // }

                println!("Serializing accounts before prev_blocks...");
                let accounts = Lazy::new(&shard_accounts)?;
                println!("Serialized accounts before prev_blocks");

                let Some(shard_balance) = shard_balance else {
                    anyhow::bail!("failed to get shard balance");
                };

                let global_balance = CurrencyCollection {
                    tokens: shard_balance.tokens + total_balance.tokens,
                    other: shard_balance
                        .other
                        .checked_add(&total_balance.other)
                        .context("Failed to add extra currency")?,
                };

                println!("Mapping masterchain extra...");
                let custom = map_mc_state_extra(
                    &custom,
                    global_config.config,
                    shard_state_id,
                    global_balance,
                )
                    .context("failed to map masterchain extra")?;
                println!("Mapped masterchain extra");

                (accounts, Some(Lazy::new(&custom)?))
            }
            None => {
                println!(
                    "Skipping extra since this state does not have it. {}",
                    old_state.seqno
                );
                (Lazy::new(&shard_accounts)?, None)
            }
        }
    };

    let mut state = ShardStateUnsplit {
        global_id: old_state.global_id,
        shard_ident: old_state.shard_ident,
        seqno: old_state.seqno,
        vert_seqno: old_state.vert_seqno,
        gen_utime: old_state.gen_utime,
        gen_utime_ms: (old_state.gen_utime % 1000) as u16,
        gen_lt: old_state.gen_lt,
        min_ref_mc_seqno: u32::MAX,
        processed_upto: ShardStateUnsplit::empty_processed_upto_info().clone(),
        before_split: false,
        accounts,
        overload_history: 0,
        underload_history: 0,
        total_balance: total_balance.clone(),
        total_validator_fees: old_state.total_validator_fees.clone(),
        libraries: old_state.libraries.clone(),
        master_ref: None,
        custom,
    };

    if current_validator_set.is_some() {
        prepare_hardfork_state(&mut state, shard_state_id, time_ms)?;
    }

    Ok((state, total_balance))
}

pub fn migrate_boc(
    bytes: &[u8],
    config: Option<GlobalConfig>,
    shard_state_id: Option<ZerostateId>,
    shard_balance: Option<CurrencyCollection>,
    current_validator_set: Option<tycho::ValidatorSet>,
    time_ms: u64,
) -> Result<(ShardStateUnsplit, CurrencyCollection)> {
    let boc = Boc::decode(bytes)?;
    println!("State cell decoded sucessfully...");
    let old_state = OldShardStateUnsplit::load_from(&mut boc.as_slice()?)
        .context("failed to load old state")?;
    println!(
        "Migrating Everscale state {}:{}",
        old_state.shard_ident, old_state.seqno,
    );
    migrate_state(
        &old_state,
        config,
        shard_state_id,
        shard_balance,
        current_validator_set,
        time_ms,
    )
}

pub fn migrate_shardstate_file(
    path: impl AsRef<Path>,
    time_ms: u64,
) -> Result<(ShardStateUnsplit, CurrencyCollection)> {
    let file = File::open(path.as_ref())?;
    // SAFETY: The file is opened read-only and the mapping does not outlive it.
    let bytes = unsafe { Mmap::map(&file)? };
    migrate_boc(&bytes, None, None, None, None, time_ms)
}

pub fn migrate_masterstate_file(
    path: impl AsRef<Path>,
    config: GlobalConfig,
    shard_state_id: ZerostateId,
    shard_balance: CurrencyCollection,
    current_validator_set: tycho::ValidatorSet,
    time_ms: u64,
) -> Result<(ShardStateUnsplit, CurrencyCollection)> {
    let file = File::open(path.as_ref())?;
    // SAFETY: The file is opened read-only and the mapping does not outlive it.
    let bytes = unsafe { Mmap::map(&file)? };
    println!("Migrating masterstate file {}", path.as_ref().display());
    migrate_boc(
        &bytes,
        Some(config),
        Some(shard_state_id),
        Some(shard_balance),
        Some(current_validator_set),
        time_ms,
    )
}

// fn migrate_file(
//     path: impl AsRef<Path>,
//     config: Option<BlockchainConfig>,
//     shard_state_hashes: Option<ZerostateId>,
//     shard_balance: Option<CurrencyCollection>,
//     current_validator_set: Option<tycho::ValidatorSet>,
//     time_ms: u64,
// ) -> Result<(ShardStateUnsplit, CurrencyCollection)> {
//     let file = File::open(path.as_ref())?;
//     // SAFETY: The file is opened read-only and the mapping does not outlive it.
//     let bytes = unsafe { Mmap::map(&file)? };
//     migrate_boc(
//         &bytes,
//         config,
//         shard_state_hashes,
//         shard_balance,
//         current_validator_set,
//         time_ms,
//     )
// }
//
fn map_shard_description(old_shard_description: &OldShardDescription) -> tycho::ShardDescription {
    tycho::ShardDescription {
        seqno: old_shard_description.seqno,
        reg_mc_seqno: old_shard_description.reg_mc_seqno,
        start_lt: old_shard_description.start_lt,
        end_lt: old_shard_description.end_lt,
        root_hash: old_shard_description.root_hash,
        file_hash: old_shard_description.file_hash,
        before_split: old_shard_description.before_split,
        before_merge: old_shard_description.before_merge,
        want_split: old_shard_description.want_split,
        want_merge: old_shard_description.want_merge,
        nx_cc_updated: true,
        next_catchain_seqno: 0,
        ext_processed_to_anchor_id: 0,
        top_sc_block_updated: false,
        min_ref_mc_seqno: u32::MAX,
        gen_utime: old_shard_description.gen_utime,
        split_merge_at: old_shard_description.split_merge_at,
        fees_collected: old_shard_description.fees_collected.clone(),
        funds_created: old_shard_description.funds_created.clone(),
    }
}

fn map_shard_hashes(
    old_shard_hashes: &tycho::ShardHashes,
    shard_state_hashes: Option<ZerostateId>,
) -> Result<tycho::ShardHashes> {
    let mut shard_entries = Vec::new();
    for i in old_shard_hashes.raw_iter() {
        let (shard_ident, mut shard_description) = i?;

        let sd = OldShardDescription::load_from(&mut shard_description)?;

        if shard_ident != ShardIdent::BASECHAIN {
            continue;
        }
        let mut new_shard_description = map_shard_description(&sd);

        if let Some(id) = shard_state_hashes {
            new_shard_description.root_hash = id.root_hash;
            new_shard_description.file_hash = id.file_hash;
        }
        shard_entries.push((shard_ident, new_shard_description));
    }

    Ok(tycho::ShardHashes::from_shards(shard_entries.iter().map(
        |(shard_ident, shard_description)| (shard_ident, shard_description),
    ))?)
}

fn override_validator_set(
    config: &mut BlockchainConfigParams,
    current_validator_set: Option<&tycho::ValidatorSet>,
) -> Result<()> {
    if let Some(vset) = current_validator_set {
        config.remove(32)?;
        config.remove(34)?;
        config.remove(35)?;
        config.remove(36)?;
        config.remove(37)?;
        config.set::<ConfigParam34>(vset)?;
    }

    Ok(())
}

fn add_validator_accounts(
    accounts: &mut ShardAccounts,
    balance_tokens: Tokens,
    validator_addresses: BTreeMap<HashBytes, String>,
) -> Result<CurrencyCollection> {
    let mut balance = Tokens::ZERO;
    for (address, data) in validator_addresses {
        let data = Boc::decode_base64(&data)?;
        let shard_account = ShardAccount::load_from(&mut data.as_slice()?)?;
        accounts.add(
            address,
            DepthBalanceInfo {
                split_depth: 0,
                balance: CurrencyCollection {
                    tokens: balance_tokens,
                    other: Default::default(),
                },
            },
            shard_account.clone(),
        )?;
        balance += balance_tokens;
    }
    Ok(CurrencyCollection {
        tokens: balance,
        other: Default::default(),
    })
}

fn prepare_hardfork_state(
    state: &mut ShardStateUnsplit,
    shardstate_id: Option<ZerostateId>,
    time_ms: u64,
) -> Result<()> {
    let state_time_ms = state.gen_utime as u64 * 1000 + state.gen_utime_ms as u64;
    if state_time_ms > time_ms {
        anyhow::bail!("Hardfork is in past");
    }
    state.gen_utime = (time_ms / 1000) as u32;
    state.gen_utime_ms = (time_ms % 1000) as u16;

    let custom = state
        .custom
        .take()
        .map(|custom| custom.load())
        .transpose()?;
    //let hardfork_cutoffs = build_hardfork_processed_to(state, custom.as_ref(), shard_state_hashes)?;
    state.processed_upto = ShardStateUnsplit::empty_processed_upto_info().clone(); //build_hardfork_processed_upto(state.seqno, &hardfork_cutoffs)?;

    let Some(mut custom) = custom else {
        return Ok(());
    };

    prepare_hardfork_mc_state_extra(
        &mut custom,
        state.seqno,
        state.gen_utime,
        state.gen_utime as u64 * 1000 + state.gen_utime_ms as u64,
        &state.total_balance,
        shardstate_id,
    )?;
    state.custom = Some(Lazy::new(&custom)?);

    Ok(())
}

// fn build_hardfork_processed_to(
//     state: &ShardStateUnsplit,
//     custom: Option<&tycho::McStateExtra>,
//     shard_state_hashes: Option<&ShardStateHashes>,
// ) -> Result<BTreeMap<tycho::ShardIdent, u64>> {
//     let mut cutoffs = BTreeMap::new();
//
//     if let Some(shard_state_hashes) = shard_state_hashes {
//         for (&shard_ident, entry) in shard_state_hashes {
//             add_hardfork_cutoff(&mut cutoffs, shard_ident, entry.gen_lt);
//         }
//     }
//
//     add_hardfork_cutoff(&mut cutoffs, state.shard_ident, state.gen_lt);
//
//     if let Some(custom) = custom {
//         add_hardfork_cutoff(&mut cutoffs, tycho::ShardIdent::MASTERCHAIN, state.gen_lt);
//
//         for entry in custom.shards.iter() {
//             let (shard_ident, shard_description) = entry?;
//             add_hardfork_cutoff(&mut cutoffs, shard_ident, shard_description.end_lt);
//         }
//     } else if !cutoffs.contains_key(&tycho::ShardIdent::MASTERCHAIN) {
//         add_hardfork_cutoff(&mut cutoffs, tycho::ShardIdent::MASTERCHAIN, state.gen_lt);
//     }
//
//     Ok(cutoffs)
// }
//
// fn add_hardfork_cutoff(
//     cutoffs: &mut BTreeMap<tycho::ShardIdent, u64>,
//     shard_ident: tycho::ShardIdent,
//     gen_lt: u64,
// ) {
//     match cutoffs.get_mut(&shard_ident) {
//         Some(existing) => *existing = (*existing).max(gen_lt),
//         None => {
//             cutoffs.insert(shard_ident, gen_lt);
//         }
//     }
// }
//
// fn build_hardfork_processed_upto(
//     state_seqno: u32,
//     cutoffs: &BTreeMap<tycho::ShardIdent, u64>,
// ) -> Result<Lazy<tycho::ProcessedUptoInfo>> {
//     let mut processed_upto = tycho::ProcessedUptoInfo::default();
//     let hardfork_range = build_hardfork_range(cutoffs)?;
//
//     for partition in HARD_FORK_PARTITIONS {
//         let mut partition_info = tycho::ProcessedUptoPartition::default();
//         partition_info.externals.processed_to = (0, 0);
//
//         for (&shard_ident, &gen_lt) in cutoffs {
//             partition_info
//                 .internals
//                 .processed_to
//                 .set(tycho::ShardIdentFull::from(shard_ident), max_processed_to(gen_lt))?;
//         }
//
//         partition_info
//             .internals
//             .ranges
//             .set(state_seqno, hardfork_range.clone())?;
//         processed_upto.partitions.set(partition, partition_info)?;
//     }
//
//     Ok(Lazy::new(&processed_upto)?)
// }
//
// fn build_hardfork_range(
//     cutoffs: &BTreeMap<tycho::ShardIdent, u64>,
// ) -> Result<tycho::InternalsRange> {
//     let mut range = tycho::InternalsRange {
//         skip_offset: 0,
//         processed_offset: 0,
//         shards: tycho_types::dict::Dict::new(),
//     };
//
//     for (&shard_ident, &gen_lt) in cutoffs {
//         range.shards.set(
//             tycho::ShardIdentFull::from(shard_ident),
//             tycho::ShardRange {
//                 from: gen_lt,
//                 to: gen_lt,
//             },
//         )?;
//     }
//
//     Ok(range)
// }
//
// fn max_processed_to(lt: u64) -> (u64, HashBytes) {
//     (lt, HashBytes([u8::MAX; 32]))
// }

fn prepare_hardfork_mc_state_extra(
    custom: &mut tycho::McStateExtra,
    mc_seqno: u32,
    gen_utime: u32,
    genesis_millis: u64,
    total_balance: &tycho::CurrencyCollection,
    shard_state_hashes: Option<ZerostateId>,
) -> Result<()> {
    let mut shards = Vec::new();
    for entry in custom.shards.iter() {
        let (ident, mut shard_description) = entry?;

        if ident != ShardIdent::BASECHAIN {
            continue;
        }

        if let Some(id) = shard_state_hashes {
            shard_description.root_hash = id.root_hash;
            shard_description.file_hash = id.file_hash;
        }

        shard_description.reg_mc_seqno = mc_seqno;
        shard_description.nx_cc_updated = true;
        shard_description.next_catchain_seqno = 0;
        shard_description.ext_processed_to_anchor_id = 0;
        shard_description.top_sc_block_updated = false;
        shard_description.min_ref_mc_seqno = u32::MAX;
        shard_description.gen_utime = gen_utime;
        shards.push((ident, shard_description));
    }

    let current_validator_set = custom.config.params.get_current_validator_set()?;
    let collation_config = custom.config.params.get_collation_config()?;
    let session_seqno = 0;
    let Some((_, validator_list_hash_short)) = current_validator_set
        .compute_mc_subset(session_seqno, collation_config.shuffle_mc_validators)
    else {
        anyhow::bail!("failed to compute validator set");
    };

    custom.shards =
        tycho::ShardHashes::from_shards(shards.iter().map(|(ident, descr)| (ident, descr)))?;
    custom.validator_info = tycho::ValidatorInfo {
        validator_list_hash_short,
        catchain_seqno: session_seqno,
        nx_cc_updated: true,
    };
    custom.consensus_info = tycho::ConsensusInfo {
        vset_switch_round: session_seqno,
        prev_vset_switch_round: session_seqno,
        genesis_info: tycho::GenesisInfo {
            start_round: 0,
            genesis_millis,
        },
        prev_shuffle_mc_validators: collation_config.shuffle_mc_validators,
    };
    custom.prev_blocks = AugDict::new();
    custom.after_key_block = true;
    custom.last_key_block = None;
    custom.block_create_stats = None;
    custom.global_balance = total_balance.clone();

    Ok(())
}

fn override_workchain_zerostates(
    params: &mut BlockchainConfigParams,
    shard_state_id: Option<ZerostateId>,
) -> Result<()> {
    let Some(id) = shard_state_id else {
        return Ok(());
    };
    let Some(mut workchains) = params.get::<tycho::ConfigParam12>()? else {
        return Ok(());
    };

    let mut updated = false;
    for entry in workchains.clone().iter() {
        let (workchain, mut description) = entry?;

        let shard_ident = tycho::ShardIdent::new_full(workchain);
        if shard_ident != ShardIdent::BASECHAIN {
            continue;
        }

        description.zerostate_root_hash = id.root_hash;
        description.zerostate_file_hash = id.file_hash;
        workchains.set(workchain, &description)?;
        updated = true;
    }

    if updated {
        params.set_workchains(&workchains)?;
    }

    Ok(())
}

// fn map_legacy_burning_config(owner_addr: HashBytes) -> tycho::BurningConfig {
//     tycho::BurningConfig {
//         blackhole_addr: Some(owner_addr),
//         fee_burn_num: 0,
//         fee_burn_denom: NonZeroU32::MIN,
//     }
// }
//
// fn default_tycho_collation_config() -> Result<tycho::CollationConfig> {
//     let mut group_slots_fractions = tycho_types::dict::Dict::<u16, u8>::new();
//     group_slots_fractions.set(0, 80)?;
//     group_slots_fractions.set(1, 10)?;
//
//     Ok(tycho::CollationConfig {
//         shuffle_mc_validators: true,
//         mc_block_min_interval_ms: 800,
//         mc_block_max_interval_ms: 2400,
//         empty_sc_block_interval_ms: 60_000,
//         max_uncommitted_chain_length: 31,
//         wu_used_to_import_next_anchor: 1_850_000_000,
//         msgs_exec_params: tycho::MsgsExecutionParams {
//             buffer_limit: 10_000,
//             group_limit: 100,
//             group_vert_size: 10,
//             externals_expire_timeout: 58,
//             open_ranges_limit: 20,
//             par_0_int_msgs_count_limit: 100_000,
//             par_0_ext_msgs_count_limit: 10_000_000,
//             group_slots_fractions,
//             range_messages_limit: 10_000,
//         },
//         work_units_params: tycho::WorkUnitsParams {
//             prepare: tycho::WorkUnitsParamsPrepare {
//                 fixed_part: 1_000_000,
//                 msgs_stats: 0,
//                 remaning_msgs_stats: 0,
//                 read_ext_msgs: 145,
//                 read_int_msgs: 2_785,
//                 read_new_msgs: 1_102,
//                 add_to_msg_groups: 80,
//             },
//             execute: tycho::WorkUnitsParamsExecute {
//                 prepare: 57_000,
//                 execute: 9_550,
//                 execute_err: 0,
//                 execute_delimiter: 1_000,
//                 serialize_enqueue: 87,
//                 serialize_dequeue: 87,
//                 insert_new_msgs: 87,
//                 subgroup_size: 16,
//             },
//             finalize: tycho::WorkUnitsParamsFinalize {
//                 build_transactions: 177,
//                 build_accounts: 275,
//                 build_in_msg: 148,
//                 build_out_msg: 145,
//                 serialize_min: 2_500_000,
//                 serialize_accounts: 3_760,
//                 serialize_msg: 3_760,
//                 state_update_min: 1_000_000,
//                 state_update_accounts: 666,
//                 state_update_msg: 425,
//                 create_diff: 1_340,
//                 serialize_diff: 105,
//                 apply_diff: 4_531,
//                 diff_tail_len: 306,
//             },
//         },
//     })
// }
//
// fn default_tycho_consensus_config() -> tycho::ConsensusConfig {
//     tycho::ConsensusConfig {
//         clock_skew_millis: NonZeroU16::new(5 * 1000).unwrap(),
//         payload_batch_bytes: NonZeroU32::new(768 * 1024).unwrap(),
//         _unused: 0,
//         commit_history_rounds: NonZeroU8::new(20).unwrap(),
//         deduplicate_rounds: 140,
//         max_consensus_lag_rounds: NonZeroU16::new(210).unwrap(),
//         payload_buffer_bytes: NonZeroU32::new(50 * 1024 * 1024).unwrap(),
//         broadcast_retry_millis: NonZeroU16::new(150).unwrap(),
//         download_retry_millis: NonZeroU16::new(25).unwrap(),
//         download_peers: NonZeroU8::new(2).unwrap(),
//         min_sign_attempts: NonZeroU8::new(3).unwrap(),
//         download_peer_queries: NonZeroU8::new(10).unwrap(),
//         sync_support_rounds: NonZeroU16::new(840).unwrap(),
//     }
// }
//
// fn default_tycho_size_limits_config() -> tycho::SizeLimitsConfig {
//     tycho::SizeLimitsConfig {
//         max_msg_bits: 1 << 21,
//         max_msg_cells: 1 << 13,
//         max_library_cells: 1000,
//         max_vm_data_depth: 512,
//         max_ext_msg_size: 65535,
//         max_ext_msg_depth: 512,
//         max_acc_state_cells: 1 << 16,
//         max_acc_state_bits: (1 << 16) * 1023,
//         max_acc_public_libraries: 256,
//         defer_out_queue_size_limit: 256,
//     }
// }
//
// fn default_tycho_global_version() -> tycho::GlobalVersion {
//     tycho::GlobalVersion {
//         version: 100,
//         capabilities: tycho::GlobalCapabilities::from([
//             tycho::GlobalCapability::CapCreateStatsEnabled,
//             tycho::GlobalCapability::CapBounceMsgBody,
//             tycho::GlobalCapability::CapReportVersion,
//             tycho::GlobalCapability::CapShortDequeue,
//             tycho::GlobalCapability::CapInitCodeHash,
//             tycho::GlobalCapability::CapOffHypercube,
//             tycho::GlobalCapability::CapFixTupleIndexBug,
//             tycho::GlobalCapability::CapFastStorageStat,
//             tycho::GlobalCapability::CapMyCode,
//             tycho::GlobalCapability::CapFullBodyInBounced,
//             tycho::GlobalCapability::CapStorageFeeToTvm,
//             tycho::GlobalCapability::CapWorkchains,
//             tycho::GlobalCapability::CapStcontNewFormat,
//             tycho::GlobalCapability::CapFastStorageStatBugfix,
//             tycho::GlobalCapability::CapResolveMerkleCell,
//             tycho::GlobalCapability::CapFeeInGasUnits,
//             tycho::GlobalCapability::CapSignatureWithId,
//             tycho::GlobalCapability::CapBounceAfterFailedAction,
//             tycho::GlobalCapability::CapSuspendedList,
//             tycho::GlobalCapability::CapsTvmBugfixes2022,
//             tycho::GlobalCapability::CapSuspendByMarks,
//             tycho::GlobalCapability::CapOmitMasterBlockHistory,
//             tycho::GlobalCapability::CapSignatureDomain,
//         ]),
//     }
// }

fn update_config_account(
    config_account: &mut tycho::ShardAccount,
    config: &tycho::BlockchainConfig,
) -> Result<()> {
    println!("Updating config contract data...");
    let Some(config_root) = config.params.as_dict().root().clone() else {
        return Err(TychoError::InvalidData.into());
    };

    let Some(mut account) = config_account.load_account()? else {
        return Ok(());
    };

    match &mut account.state {
        tycho::AccountState::Active(state) => {
            let mut builder = CellBuilder::new();
            builder.store_reference(config_root)?;

            if let Some(data) = state.data.take() {
                let mut data = data.as_slice()?;
                data.load_reference()?;
                builder.store_slice(data)?;
            }

            state.data = Some(builder.build()?);
        }
        _ => anyhow::bail!("invalid config state"),
    }

    config_account.account = Lazy::new(&tycho::OptionalAccount(Some(account)))?;
    Ok(())
}

fn reset_elector_account(account: &mut tycho::ShardAccount) -> Result<()> {
    static ELECTOR_ABI: OnceLock<AbiType> = OnceLock::new();

    let Some(mut loaded_account) = account.load_account()? else {
        return Err(TychoError::InvalidData.into());
    };

    let grams = loaded_account.balance.tokens;
    let state = match &mut loaded_account.state {
        AccountState::Active(state) => state,
        _ => anyhow::bail!("invalid elector state"),
    };
    let elector_data = state.data.as_ref().context("elector data is empty")?;

    let abi_type = ELECTOR_ABI.get_or_init(PartialElectorData::abi_type);
    let mut elector_data = elector_data.as_slice()?;
    let mut data = AbiValue::load_partial(abi_type, AbiVersion::V2_1, &mut elector_data)
        .and_then(PartialElectorData::from_abi)?;
    data.current_election = None;
    data.past_elections = BTreeMap::new();
    data.active_hash = HashBytes::default();
    data.credits = BTreeMap::new();
    data.grams = grams;
    data.active_id = 0;

    let mut builder = CellBuilder::new();
    let elector_data_prefix = data.as_abi().make_cell(AbiVersion::V2_1)?;
    builder.store_slice(elector_data_prefix.as_slice()?)?;
    builder.store_slice(elector_data)?;
    state.data = Some(builder.build()?);
    account.account = Lazy::new(&tycho::OptionalAccount(Some(loaded_account)))?;

    Ok(())
}

fn map_mc_state_extra(
    old_mc_state_extra: &OldMcStateExtra,
    config: BlockchainConfig,
    shard_state_id: Option<ZerostateId>,
    global_balance: CurrencyCollection,
) -> Result<tycho::McStateExtra> {
    println!("MC extra: mapping shards...");

    let shards = map_shard_hashes(&old_mc_state_extra.shards, shard_state_id)
        .context("failed to map shard hashes")?;

    let validator_info = tycho::ValidatorInfo {
        validator_list_hash_short: old_mc_state_extra.validator_info.validator_list_hash_short,
        catchain_seqno: old_mc_state_extra.validator_info.catchain_seqno,
        nx_cc_updated: old_mc_state_extra.validator_info.nx_cc_updated,
    };

    Ok(tycho::McStateExtra {
        shards,
        config,
        validator_info,
        consensus_info: tycho::ConsensusInfo::ZEROSTATE,
        prev_blocks: AugDict::new(),
        after_key_block: old_mc_state_extra.after_key_block,
        last_key_block: old_mc_state_extra.last_key_block.clone(),
        block_create_stats: old_mc_state_extra.block_create_stats.clone(),
        global_balance,
    })
}
