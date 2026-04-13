use std::fs::File;
use std::num::{NonZeroU8, NonZeroU16, NonZeroU32};
use std::path::Path;

use memmap2::Mmap;
use thiserror::Error;
use ton_block::{Deserializable as _, HashmapAugType as _};
use ton_types::HashmapType as _;
use tycho_types::boc::{self, Boc, BocRepr, BocReprError};
use tycho_types::cell::{CellBuilder, CellFamily as _, Lazy, Load};
use tycho_types::error::Error as TychoError;
use tycho_types::models as tycho;
use tycho_types::models::BlockchainConfigParams;

pub type Result<T> = std::result::Result<T, MigrationError>;

#[derive(Debug, Error)]
pub enum MigrationError {
    #[error("failed to read shard state file: {0}")]
    Io(#[from] std::io::Error),
    #[error("failed to decode old shard state: {0}")]
    Old(#[from] ton_types::Error),
    #[error("failed to decode bridged BOC: {0}")]
    TychoBoc(#[from] boc::de::Error),
    #[error("failed to parse bridged tycho value: {0}")]
    TychoRepr(#[from] BocReprError),
    #[error("failed to build tycho shard state: {0}")]
    Tycho(#[from] TychoError),
}

pub fn migrate_state(old_state: &ton_block::ShardStateUnsplit) -> Result<tycho::ShardStateUnsplit> {
    // Convert config first so we can patch the config account and drop the
    // fully materialized accounts dict before heavyweight mc extra mapping.
    let (accounts, custom) = {
        let mut accounts = map_shard_accounts(&old_state.read_accounts()?)?;
        match old_state.read_custom()? {
            Some(custom) => {
                println!("Mapping masterchain config...");
                let config = map_blockchain_config(&custom.config, old_state.global_id())?;
                println!("Mapped masterchain config");

                update_config_account(&mut accounts, &config)?;

                println!("Serializing accounts before prev_blocks...");
                let accounts = Lazy::new(&accounts)?;
                println!("Serialized accounts before prev_blocks");

                println!("Mapping masterchain extra...");
                let custom = map_mc_state_extra(&custom, config)?;
                println!("Mapped masterchain extra");

                (accounts, Some(Lazy::new(&custom)?))
            }
            None => {
                println!("Skipping extra since this state does not have it. {}", old_state.seq_no());
                (Lazy::new(&accounts)?, None)
            }
        }
    };

    println!("Mapping state libraries...");
    let libraries = map_libraries(old_state.libraries())?;
    println!("Mapped state libraries");

    Ok(tycho::ShardStateUnsplit {
        global_id: old_state.global_id(),
        shard_ident: map_shard_ident(old_state.shard())?,
        seqno: old_state.seq_no(),
        vert_seqno: old_state.vert_seq_no(),
        gen_utime: old_state.gen_time(),
        gen_utime_ms: 0,
        gen_lt: old_state.gen_lt(),
        min_ref_mc_seqno: old_state.min_ref_mc_seqno(),
        processed_upto: Lazy::new(&tycho::ProcessedUptoInfo::default())?, // Old shard states do not contain tycho's processed-up-to data.
        before_split: old_state.before_split(),
        accounts,
        overload_history: old_state.overload_history(),
        underload_history: old_state.underload_history(),
        total_balance: convert_via_boc(old_state.total_balance())?,
        total_validator_fees: convert_via_boc(old_state.total_validator_fees())?,
        libraries,
        master_ref: old_state
            .master_ref()
            .map(|master_ref| map_block_ref(&master_ref.master)),
        custom,
    })
}

pub fn migrate_boc(bytes: &[u8]) -> Result<tycho::ShardStateUnsplit> {
    let old_state = ton_block::ShardStateUnsplit::construct_from_bytes(bytes)?;
    println!("Migrating Everscale shard state {}", old_state.id());
    migrate_state(&old_state)
}

pub fn migrate_file(path: impl AsRef<Path>) -> Result<tycho::ShardStateUnsplit> {
    let file = File::open(path.as_ref())?;
    // SAFETY: The file is opened read-only and the mapping does not outlive it.
    let bytes = unsafe { Mmap::map(&file)? };
    migrate_boc(&bytes)
}

fn convert_via_boc<T, O>(value: &O) -> Result<T>
where
    O: ton_block::Serializable,
    for<'a> T: Load<'a>,
{
    let bytes = value.write_to_bytes()?;
    Ok(BocRepr::decode(bytes.as_slice())?)
}

fn convert_hash(hash: &ton_types::UInt256) -> tycho_types::cell::HashBytes {
    (*hash.as_slice()).into()
}

fn convert_old_cell(cell: &ton_types::Cell) -> Result<tycho_types::cell::Cell> {
    let bytes = ton_types::serialize_toc(cell)?;
    Ok(Boc::decode(bytes.as_slice())?)
}

fn map_shard_ident(old_shard_ident: &ton_block::ShardIdent) -> Result<tycho::ShardIdent> {
    convert_via_boc(old_shard_ident)
}

fn map_block_ref(old_block_ref: &ton_block::ExtBlkRef) -> tycho::BlockRef {
    tycho::BlockRef {
        end_lt: old_block_ref.end_lt,
        seqno: old_block_ref.seq_no,
        root_hash: convert_hash(&old_block_ref.root_hash),
        file_hash: convert_hash(&old_block_ref.file_hash),
    }
}

fn map_future_split_merge(
    old_split_merge: &ton_block::FutureSplitMerge,
) -> Option<tycho::FutureSplitMerge> {
    match old_split_merge {
        ton_block::FutureSplitMerge::None => None,
        ton_block::FutureSplitMerge::Split {
            split_utime,
            interval,
        } => Some(tycho::FutureSplitMerge::Split {
            split_utime: *split_utime,
            interval: *interval,
        }),
        ton_block::FutureSplitMerge::Merge {
            merge_utime,
            interval,
        } => Some(tycho::FutureSplitMerge::Merge {
            merge_utime: *merge_utime,
            interval: *interval,
        }),
    }
}

fn map_shard_description(
    old_shard_description: &ton_block::ShardDescr,
) -> Result<tycho::ShardDescription> {
    Ok(tycho::ShardDescription {
        seqno: old_shard_description.seq_no,
        reg_mc_seqno: old_shard_description.reg_mc_seqno,
        start_lt: old_shard_description.start_lt,
        end_lt: old_shard_description.end_lt,
        root_hash: convert_hash(&old_shard_description.root_hash),
        file_hash: convert_hash(&old_shard_description.file_hash),
        before_split: old_shard_description.before_split,
        before_merge: old_shard_description.before_merge,
        want_split: old_shard_description.want_split,
        want_merge: old_shard_description.want_merge,
        nx_cc_updated: old_shard_description.nx_cc_updated,
        next_catchain_seqno: old_shard_description.next_catchain_seqno,
        ext_processed_to_anchor_id: 0, // set to zero like it is used in zerostate creation
        top_sc_block_updated: false, // Old shard descriptions do not contain `top_sc_block_updated`,
        min_ref_mc_seqno: old_shard_description.min_ref_mc_seqno,
        gen_utime: old_shard_description.gen_utime,
        split_merge_at: map_future_split_merge(&old_shard_description.split_merge_at),
        fees_collected: convert_via_boc(&old_shard_description.fees_collected)?,
        funds_created: convert_via_boc(&old_shard_description.funds_created)?,
    })
}

fn map_shard_hashes(old_shard_hashes: &ton_block::ShardHashes) -> Result<tycho::ShardHashes> {
    let mut shard_entries = Vec::new();
    old_shard_hashes.iterate_shards(|old_shard_ident, old_shard_description| {
        shard_entries.push((
            map_shard_ident(&old_shard_ident)?,
            map_shard_description(&old_shard_description)?,
        ));
        Ok(true)
    })?;

    Ok(tycho::ShardHashes::from_shards(shard_entries.iter().map(
        |(shard_ident, shard_description)| (shard_ident, shard_description),
    ))?)
}

fn map_blockchain_config(
    old_config: &ton_block::ConfigParams,
    global_id: i32,
) -> Result<tycho::BlockchainConfig> {
    let address = convert_hash(&old_config.config_addr);
    let mut config = match old_config.config_params.data() {
        Some(root) => tycho::BlockchainConfig {
            address,
            params: BlockchainConfigParams::from_raw(convert_old_cell(root)?),
        },
        None => tycho::BlockchainConfig::new_empty(address),
    };
    map_blockchain_config_params(&mut config.params, old_config, global_id)?;
    Ok(config)
}

fn map_legacy_burning_config(owner_addr: &ton_types::UInt256) -> tycho::BurningConfig {
    tycho::BurningConfig {
        blackhole_addr: Some(convert_hash(owner_addr)),
        fee_burn_num: 0,
        fee_burn_denom: NonZeroU32::MIN,
    }
}

fn default_tycho_collation_config() -> Result<tycho::CollationConfig> {
    let mut group_slots_fractions = tycho_types::dict::Dict::<u16, u8>::new();
    group_slots_fractions.set(0, 80)?;
    group_slots_fractions.set(1, 10)?;

    Ok(tycho::CollationConfig {
        shuffle_mc_validators: true,
        mc_block_min_interval_ms: 800,
        mc_block_max_interval_ms: 2400,
        empty_sc_block_interval_ms: 60_000,
        max_uncommitted_chain_length: 31,
        wu_used_to_import_next_anchor: 1_850_000_000,
        msgs_exec_params: tycho::MsgsExecutionParams {
            buffer_limit: 10_000,
            group_limit: 100,
            group_vert_size: 10,
            externals_expire_timeout: 58,
            open_ranges_limit: 20,
            par_0_int_msgs_count_limit: 100_000,
            par_0_ext_msgs_count_limit: 10_000_000,
            group_slots_fractions,
            range_messages_limit: 10_000,
        },
        work_units_params: tycho::WorkUnitsParams {
            prepare: tycho::WorkUnitsParamsPrepare {
                fixed_part: 1_000_000,
                msgs_stats: 0,
                remaning_msgs_stats: 0,
                read_ext_msgs: 145,
                read_int_msgs: 2_785,
                read_new_msgs: 1_102,
                add_to_msg_groups: 80,
            },
            execute: tycho::WorkUnitsParamsExecute {
                prepare: 57_000,
                execute: 9_550,
                execute_err: 0,
                execute_delimiter: 1_000,
                serialize_enqueue: 87,
                serialize_dequeue: 87,
                insert_new_msgs: 87,
                subgroup_size: 16,
            },
            finalize: tycho::WorkUnitsParamsFinalize {
                build_transactions: 177,
                build_accounts: 275,
                build_in_msg: 148,
                build_out_msg: 145,
                serialize_min: 2_500_000,
                serialize_accounts: 3_760,
                serialize_msg: 3_760,
                state_update_min: 1_000_000,
                state_update_accounts: 666,
                state_update_msg: 425,
                create_diff: 1_340,
                serialize_diff: 105,
                apply_diff: 4_531,
                diff_tail_len: 306,
            },
        },
    })
}

fn default_tycho_consensus_config() -> tycho::ConsensusConfig {
    tycho::ConsensusConfig {
        clock_skew_millis: NonZeroU16::new(5 * 1000).unwrap(),
        payload_batch_bytes: NonZeroU32::new(768 * 1024).unwrap(),
        _unused: 0,
        commit_history_rounds: NonZeroU8::new(20).unwrap(),
        deduplicate_rounds: 140,
        max_consensus_lag_rounds: NonZeroU16::new(210).unwrap(),
        payload_buffer_bytes: NonZeroU32::new(50 * 1024 * 1024).unwrap(),
        broadcast_retry_millis: NonZeroU16::new(150).unwrap(),
        download_retry_millis: NonZeroU16::new(25).unwrap(),
        download_peers: NonZeroU8::new(2).unwrap(),
        min_sign_attempts: NonZeroU8::new(3).unwrap(),
        download_peer_queries: NonZeroU8::new(10).unwrap(),
        sync_support_rounds: NonZeroU16::new(840).unwrap(),
    }
}

fn default_tycho_size_limits_config() -> tycho::SizeLimitsConfig {
    tycho::SizeLimitsConfig {
        max_msg_bits: 1 << 21,
        max_msg_cells: 1 << 13,
        max_library_cells: 1000,
        max_vm_data_depth: 512,
        max_ext_msg_size: 65535,
        max_ext_msg_depth: 512,
        max_acc_state_cells: 1 << 16,
        max_acc_state_bits: (1 << 16) * 1023,
        max_acc_public_libraries: 256,
        defer_out_queue_size_limit: 256,
    }
}

fn map_blockchain_config_params(
    params: &mut BlockchainConfigParams,
    old_params: &ton_block::ConfigParams,
    global_id: i32,
) -> Result<()> {
    if let Some(ton_block::ConfigParamEnum::ConfigParam5(old_burning)) = old_params.config(5)? {
        params.set_burning_config(&map_legacy_burning_config(&old_burning.owner_addr))?;
    }

    params.set_collation_config(&default_tycho_collation_config()?)?;

    params.set_consensus_config(&default_tycho_consensus_config())?;
    params.set_global_id(global_id)?;
    params.set_size_limits(&default_tycho_size_limits_config())?;

    params.remove(50)?;
    params.remove(100)?;

    Ok(())
}

fn update_config_account(
    accounts: &mut tycho::ShardAccounts,
    config: &tycho::BlockchainConfig,
) -> Result<()> {
    println!("Updating config contract data...");
    let Some(config_root) = config.params.as_dict().root().clone() else {
        return Err(TychoError::InvalidData.into());
    };

    let Some((depth_balance, mut shard_account)) = accounts.get(config.address)? else {
        return Ok(());
    };

    let Some(mut account) = shard_account.load_account()? else {
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
        tycho::AccountState::Uninit | tycho::AccountState::Frozen(..) => return Ok(()),
    }

    shard_account.account = Lazy::new(&tycho::OptionalAccount(Some(account)))?;
    accounts.set(config.address, depth_balance, shard_account)?;
    println!("Config contract was updated!");

    Ok(())
}

fn map_prev_blocks(
    old_prev_blocks: &ton_block::OldMcBlocksInfo,
) -> Result<tycho_types::dict::AugDict<u32, tycho::KeyMaxLt, tycho::KeyBlockRef>> {
    let Some(root) = old_prev_blocks.data().cloned() else {
        return Ok(tycho_types::dict::AugDict::new());
    };

    let root = convert_old_cell(&root)?;
    let mut slice = root.as_slice()?;
    let dict = tycho_types::dict::AugDict::load_from_root_ext(
        &mut slice,
        tycho_types::cell::Cell::empty_context(),
    )?;
    println!("Converted prev blocks");
    Ok(dict)
}

fn map_libraries(
    old_libraries: &ton_block::Libraries,
) -> Result<tycho_types::dict::Dict<tycho_types::cell::HashBytes, tycho::LibDescr>> {
    let Some(root) = old_libraries.root().cloned() else {
        return Ok(tycho_types::dict::Dict::new());
    };

    let root = convert_old_cell(&root)?;
    let mut slice = root.as_slice()?;
    let dict = tycho_types::dict::Dict::load_from_root_ext(
        &mut slice,
        tycho_types::cell::Cell::empty_context(),
    )?;
    println!("Converted libraries");
    Ok(dict)
}

fn map_mc_state_extra(
    old_mc_state_extra: &ton_block::McStateExtra,
    config: tycho::BlockchainConfig,
) -> Result<tycho::McStateExtra> {
    println!("MC extra: mapping shards...");
    let shards = map_shard_hashes(&old_mc_state_extra.shards)?;
    println!("MC extra: mapped shards");

    println!("MC extra: mapping validator info...");
    let validator_info = tycho::ValidatorInfo {
        validator_list_hash_short: old_mc_state_extra.validator_info.validator_list_hash_short,
        catchain_seqno: old_mc_state_extra.validator_info.catchain_seqno,
        nx_cc_updated: old_mc_state_extra.validator_info.nx_cc_updated,
    };
    println!("MC extra: mapped validator info");

    println!("MC extra: mapping prev blocks...");
    let prev_blocks = map_prev_blocks(&old_mc_state_extra.prev_blocks)?;
    println!("MC extra: mapped prev blocks");

    println!("MC extra: mapping block create stats...");
    let block_create_stats = map_block_create_stats(old_mc_state_extra.block_create_stats.as_ref())?;
    println!("MC extra: mapped block create stats");

    println!("MC extra: mapping global balance...");
    let global_balance = convert_via_boc(&old_mc_state_extra.global_balance)?;
    println!("MC extra: mapped global balance");

    Ok(tycho::McStateExtra {
        shards,
        config,
        validator_info,
        consensus_info: tycho::ConsensusInfo::ZEROSTATE,
        prev_blocks,
        after_key_block: old_mc_state_extra.after_key_block,
        last_key_block: old_mc_state_extra
            .last_key_block
            .as_ref()
            .map(map_block_ref),
        block_create_stats,
        global_balance,
    })
}

fn map_block_create_stats(
    old_block_create_stats: Option<&ton_block::BlockCreateStats>,
) -> Result<Option<tycho_types::dict::Dict<tycho_types::cell::HashBytes, tycho::CreatorStats>>> {
    let Some(old_block_create_stats) = old_block_create_stats else {
        return Ok(None);
    };

    let Some(root) = old_block_create_stats.counters.root().cloned() else {
        return Ok(Some(tycho_types::dict::Dict::new()));
    };

    let root = convert_old_cell(&root)?;
    let mut slice = root.as_slice()?;
    let dict = tycho_types::dict::Dict::load_from_root_ext(
        &mut slice,
        tycho_types::cell::Cell::empty_context(),
    )?;
    Ok(Some(dict))
}

fn map_account(old_account: &ton_block::Account) -> Result<Option<tycho::Account>> {
    let Some(old_account_stuff) = old_account.stuff() else {
        return Ok(None);
    };

    if old_account_stuff.storage.init_code_hash.is_some() {
        // TODO: tycho types has no equivalent field
    }

    Ok(Some(tycho::Account {
        address: convert_via_boc(&old_account_stuff.addr)?,
        storage_stat: convert_via_boc(&old_account_stuff.storage_stat)?,
        last_trans_lt: old_account_stuff.storage.last_trans_lt,
        balance: convert_via_boc(&old_account_stuff.storage.balance)?,
        state: convert_via_boc(&old_account_stuff.storage.state)?,
    }))
}

fn depth_balance_from_account(account: Option<&tycho::Account>) -> tycho::DepthBalanceInfo {
    match account {
        Some(account) => tycho::DepthBalanceInfo {
            split_depth: match &account.state {
                tycho::AccountState::Active(state_init) => state_init
                    .split_depth
                    .map(|depth| depth.into_bit_len() as u8)
                    .unwrap_or_default(),
                tycho::AccountState::Frozen(_) | tycho::AccountState::Uninit => 0,
            },
            balance: account.balance.clone(),
        },
        None => tycho::DepthBalanceInfo::default(),
    }
}

fn map_shard_account(
    old_shard_account: &ton_block::ShardAccount,
) -> Result<(tycho::DepthBalanceInfo, tycho::ShardAccount)> {
    let account = map_account(&old_shard_account.read_account()?)?;
    let depth_balance_info = depth_balance_from_account(account.as_ref());
    let account = tycho::OptionalAccount(account);

    Ok((
        depth_balance_info,
        tycho::ShardAccount {
            account: Lazy::new(&account)?,
            last_trans_hash: convert_hash(old_shard_account.last_trans_hash()),
            last_trans_lt: old_shard_account.last_trans_lt(),
        },
    ))
}

fn map_shard_accounts(old_accounts: &ton_block::ShardAccounts) -> Result<tycho::ShardAccounts> {
    let mut accounts = tycho::ShardAccounts::new();
    let mut total_account = 0u32;
    println!("Converting accounts...");
    old_accounts.iterate_with_keys(|account_id: ton_types::UInt256, old_shard_account| {
        let (depth_balance_info, shard_account) = map_shard_account(&old_shard_account)?;
        accounts.set(convert_hash(&account_id), depth_balance_info, shard_account)?;
        total_account += 1;
        Ok(true)
    })?;
    println!("Total accounts converted: {}", total_account);
    Ok(accounts)
}
