use std::fs::File;
use std::num::{NonZeroU8, NonZeroU16, NonZeroU32};
use std::path::Path;

use memmap2::Mmap;
use thiserror::Error;
use ton_block::{Deserializable as _, HashmapAugType as _};
use ton_types::HashmapType as _;
use tycho_types::boc::{self, Boc, BocRepr, BocReprError};
use tycho_types::cell::{Lazy, Load};
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
    let custom = old_state
        .read_custom()?
        .map(|custom| map_mc_state_extra(&custom, old_state.global_id()))
        .transpose()?
        .map(|custom| Lazy::new(&custom))
        .transpose()?;

    let accounts = map_shard_accounts(&old_state.read_accounts()?)?;

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
        accounts: Lazy::new(&accounts)?,
        overload_history: old_state.overload_history(),
        underload_history: old_state.underload_history(),
        total_balance: convert_via_boc(old_state.total_balance())?,
        total_validator_fees: convert_via_boc(old_state.total_validator_fees())?,
        libraries: convert_via_boc(old_state.libraries())?,
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
    let root = old_config.config_params.data().cloned().unwrap_or_default();
    let config_params = ton_block::config_params::ConfigParams::with_root(root);
    let mut config = tycho::BlockchainConfig::new_empty(convert_hash(&old_config.config_addr));
    map_blockchain_config_params(&mut config.params, config_params, global_id)?;
    Ok(config)
}

fn copy_old_blockchain_config_params(
    params: &mut BlockchainConfigParams,
    old_params: &ton_block::ConfigParams,
) -> Result<()> {
    let mut old_param_cells = Vec::new();
    old_params.config_params.iterate_slices(|mut key, value| {
        let id = u32::construct_from(&mut key)?;
        if let Some(cell) = value.reference_opt(0) {
            old_param_cells.push((id, cell));
        }
        Ok(true)
    })?;

    for (id, cell) in old_param_cells {
        params.set_raw(id, convert_old_cell(&cell)?)?;
    }

    Ok(())
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
    old_params: ton_block::ConfigParams,
    global_id: i32,
) -> Result<()> {
    copy_old_blockchain_config_params(params, &old_params)?;

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

fn map_mc_state_extra(
    old_mc_state_extra: &ton_block::McStateExtra,
    global_id: i32,
) -> Result<tycho::McStateExtra> {
    Ok(tycho::McStateExtra {
        shards: map_shard_hashes(&old_mc_state_extra.shards)?,
        config: map_blockchain_config(&old_mc_state_extra.config, global_id)?,
        validator_info: convert_via_boc(&old_mc_state_extra.validator_info)?,
        consensus_info: tycho::ConsensusInfo::ZEROSTATE,
        prev_blocks: convert_via_boc(&old_mc_state_extra.prev_blocks)?,
        after_key_block: old_mc_state_extra.after_key_block,
        last_key_block: old_mc_state_extra
            .last_key_block
            .as_ref()
            .map(map_block_ref),
        block_create_stats: old_mc_state_extra
            .block_create_stats
            .as_ref()
            .map(|block_create_stats| convert_via_boc(&block_create_stats.counters))
            .transpose()?,
        global_balance: convert_via_boc(&old_mc_state_extra.global_balance)?,
    })
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
