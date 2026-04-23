use tycho_types::cell::{Cell, CellSlice, HashBytes, Lazy, Load};
use tycho_types::dict::{AugDict, Dict};
use tycho_types::error::Error;
use tycho_types::models::{
    BlockRef, BlockchainConfig, CreatorStats, CurrencyCollection, FutureSplitMerge, KeyBlockRef,
    KeyMaxLt, LibDescr, ShardAccounts, ShardHashes, ShardIdent, ValidatorInfo,
};

macro_rules! ok {
    ($e:expr $(,)?) => {
        match $e {
            core::result::Result::Ok(val) => val,
            core::result::Result::Err(err) => return core::result::Result::Err(err),
        }
    };
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct OldShardStateUnsplit {
    /// Global network id.
    pub global_id: i32,
    /// Id of the shard.
    pub shard_ident: ShardIdent,
    /// Sequence number of the corresponding block.
    pub seqno: u32,
    /// Vertical sequcent number of the corresponding block.
    pub vert_seqno: u32,
    /// Unix timestamp when the block was created.
    pub gen_utime: u32,
    /// Logical time when the state was created.
    pub gen_lt: u64,
    /// Minimal referenced seqno of the masterchain block.
    pub min_ref_mc_seqno: u32,
    /// Output messages queue info (stub).
    pub out_msg_queue_info: Cell,
    /// Whether this state was produced before the shards split.
    pub before_split: bool,
    /// Reference to the dictionary with shard accounts.
    pub accounts: Lazy<ShardAccounts>,
    /// Mask for the overloaded blocks.
    pub overload_history: u64,
    /// Mask for the underloaded blocks.
    pub underload_history: u64,
    /// Total balance for all currencies.
    pub total_balance: CurrencyCollection,
    /// Total pending validator fees.
    pub total_validator_fees: CurrencyCollection,
    /// Dictionary with all libraries and its providers.
    pub libraries: Dict<HashBytes, LibDescr>,
    /// Optional reference to the masterchain block.
    pub master_ref: Option<BlockRef>,
    /// Shard state additional info.
    pub custom: Option<Lazy<OldMcStateExtra>>,
}

impl OldShardStateUnsplit {
    const TAG_V1: u32 = 0x9023afe2;

    /// Tries to load shard accounts dictionary.
    pub fn load_accounts(&self) -> Result<ShardAccounts, Error> {
        self.accounts.load()
    }

    /// Tries to load additional masterchain data.
    pub fn load_custom(&self) -> Result<Option<OldMcStateExtra>, Error> {
        match &self.custom {
            Some(custom) => match custom.load() {
                Ok(custom) => Ok(Some(custom)),
                Err(e) => Err(e),
            },
            None => Ok(None),
        }
    }

    // /// Tries to set additional masterchain data.
    // pub fn set_custom(&mut self, value: Option<&McStateExtra>) -> Result<(), Error> {
    //     match (&mut self.custom, value) {
    //         (None, None) => Ok(()),
    //         (None, Some(value)) => {
    //             self.custom = Some(ok!(Lazy::new(value)));
    //             Ok(())
    //         }
    //         (Some(_), None) => {
    //             self.custom = None;
    //             Ok(())
    //         }
    //         (Some(custom), Some(value)) => custom.set(value),
    //     }
    // }
}

impl<'a> Load<'a> for OldShardStateUnsplit {
    fn load_from(slice: &mut CellSlice<'a>) -> Result<Self, Error> {
        let fast_finality = match slice.load_u32() {
            Ok(Self::TAG_V1) => false,
            Ok(_) => return Err(Error::InvalidTag),
            Err(e) => return Err(e),
        };

        let _ = fast_finality;

        let out_msg_queue_info = ok!(<_>::load_from(slice));

        let accounts = ok!(Lazy::load_from(slice));

        let child_slice = &mut ok!(slice.load_reference_as_slice());

        let global_id = ok!(slice.load_u32()) as i32;
        let shard_ident = ok!(ShardIdent::load_from(slice));

        Ok(Self {
            global_id,
            shard_ident,
            seqno: ok!(slice.load_u32()),
            vert_seqno: ok!(slice.load_u32()),
            gen_utime: ok!(slice.load_u32()),
            gen_lt: ok!(slice.load_u64()),
            min_ref_mc_seqno: ok!(slice.load_u32()),
            before_split: ok!(slice.load_bit()),
            accounts,
            overload_history: ok!(child_slice.load_u64()),
            underload_history: ok!(child_slice.load_u64()),
            total_balance: ok!(CurrencyCollection::load_from(child_slice)),
            total_validator_fees: ok!(CurrencyCollection::load_from(child_slice)),
            libraries: ok!(Dict::load_from(child_slice)),
            master_ref: ok!(Option::<BlockRef>::load_from(child_slice)),
            out_msg_queue_info,
            #[allow(unused_labels)]
            custom: ok!(Option::<Lazy<OldMcStateExtra>>::load_from(slice)),
        })
    }
}

#[derive(Debug, Clone)]
pub struct OldMcStateExtra {
    /// A tree of the most recent descriptions for all currently existing shards
    /// for all workchains except the masterchain.
    pub shards: ShardHashes,
    /// The most recent blockchain config (if the block is a key block).
    pub config: BlockchainConfig,
    /// Brief validator info.
    pub validator_info: ValidatorInfo,
    /// A dictionary with previous masterchain blocks.
    pub prev_blocks: AugDict<u32, KeyMaxLt, KeyBlockRef>,
    /// Whether this state was produced after the key block.
    pub after_key_block: bool,
    /// Optional reference to the latest known key block.
    pub last_key_block: Option<BlockRef>,
    /// Block creation stats for validators from the current set.
    pub block_create_stats: Option<Dict<HashBytes, CreatorStats>>,
    /// Total balance of all accounts.
    pub global_balance: CurrencyCollection,
}

impl OldMcStateExtra {
    const TAG: u16 = 0xcc26;
    const BLOCK_STATS_TAG: u8 = 0x17;
}

impl<'a> Load<'a> for OldMcStateExtra {
    fn load_from(slice: &mut CellSlice<'a>) -> Result<Self, Error> {
        match slice.load_u16() {
            Ok(Self::TAG) => {}
            Ok(_) => return Err(Error::InvalidTag),
            Err(e) => return Err(e),
        }

        let shards = ok!(ShardHashes::load_from(slice));
        let config = ok!(BlockchainConfig::load_from(slice));

        let child_slice = &mut ok!(slice.load_reference_as_slice());
        let flags = ok!(child_slice.load_u16());

        const RESERVED_BITS: usize = 2;

        if flags >> RESERVED_BITS != 0 {
            return Err(Error::InvalidData);
        }

        Ok(Self {
            shards,
            config,
            validator_info: ok!(ValidatorInfo::load_from(child_slice)),
            prev_blocks: ok!(AugDict::load_from(child_slice)),
            after_key_block: ok!(child_slice.load_bit()),
            last_key_block: ok!(Option::<BlockRef>::load_from(child_slice)),
            block_create_stats: if flags & 0b001 != 0 {
                if ok!(child_slice.load_u8()) != Self::BLOCK_STATS_TAG {
                    return Err(Error::InvalidTag);
                }
                Some(ok!(Dict::load_from(child_slice)))
            } else {
                None
            },
            global_balance: ok!(CurrencyCollection::load_from(slice)),
        })
    }
}

/// Description of the most recent state of the shard.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct OldShardDescription {
    /// Sequence number of the latest block in the shard.
    pub seqno: u32,
    /// The latest known masterchain block at the time of shard generation.
    pub reg_mc_seqno: u32,
    /// The beginning of the logical time range since the last MC block.
    pub start_lt: u64,
    /// The end of the logical time range since the last MC block.
    pub end_lt: u64,
    /// Representation hash of the root cell of the latest block in the shard.
    pub root_hash: HashBytes,
    /// Hash of the BOC encoded root cell of the latest block in the shard.
    pub file_hash: HashBytes,
    /// Whether this shard splits in the next block.
    pub before_split: bool,
    /// Whether this shard merges in the next block.
    pub before_merge: bool,
    /// Hint that this shard should split.
    pub want_split: bool,
    /// Hint that this shard should merge.
    pub want_merge: bool,
    /// Whether the value of catchain seqno has been incremented
    /// and will it also be incremented in the next block.
    pub nx_cc_updated: bool,
    /// Catchain seqno in the next block.
    pub next_catchain_seqno: u32,
    /// Duplicates the shard ident for the latest block in this shard.
    pub next_validator_shard: u64,
    /// Minimal referenced seqno of the masterchain block.
    pub min_ref_mc_seqno: u32,
    /// Unix timestamp when the latest block in this shard was created.
    pub gen_utime: u32,
    /// Planned split/merge time window if present.
    pub split_merge_at: Option<FutureSplitMerge>,
    /// Amount of fees collected in this shard since the last masterchain block.
    pub fees_collected: CurrencyCollection,
    /// Amount of funds created in this shard since the last masterchain block.
    pub funds_created: CurrencyCollection,
}

impl OldShardDescription {
    const TAG_LEN: u16 = 4;

    const TAG_V1: u8 = 0xa;
    const TAG_V2: u8 = 0xb;
}

impl<'a> Load<'a> for OldShardDescription {
    fn load_from(slice: &mut CellSlice<'a>) -> Result<Self, Error> {
        let cont_in_cell = match slice.load_small_uint(Self::TAG_LEN) {
            Ok(Self::TAG_V1) => true,
            Ok(Self::TAG_V2) => false,
            Ok(_) => return Err(Error::InvalidTag),
            Err(e) => return Err(e),
        };

        let seqno = ok!(slice.load_u32());
        let reg_mc_seqno = ok!(slice.load_u32());
        let start_lt = ok!(slice.load_u64());
        let end_lt = ok!(slice.load_u64());
        let root_hash = ok!(slice.load_u256());
        let file_hash = ok!(slice.load_u256());

        let flags = ok!(slice.load_u8());
        if flags & 0b11 != 0 {
            return Err(Error::InvalidData);
        }

        let next_catchain_seqno = ok!(slice.load_u32());
        let next_validator_shard = ok!(slice.load_u64());
        let min_ref_mc_seqno = ok!(slice.load_u32());
        let gen_utime = ok!(slice.load_u32());
        let split_merge_at = ok!(Option::<FutureSplitMerge>::load_from(slice));

        let mut cont = if cont_in_cell {
            Some(ok!(slice.load_reference_as_slice()))
        } else {
            None
        };

        let slice = match &mut cont {
            Some(cont) => cont,
            None => slice,
        };

        let fees_collected = ok!(CurrencyCollection::load_from(slice));
        let funds_created = ok!(CurrencyCollection::load_from(slice));

        Ok(Self {
            seqno,
            reg_mc_seqno,
            start_lt,
            end_lt,
            root_hash,
            file_hash,
            before_split: flags & 0b10000000 != 0,
            before_merge: flags & 0b01000000 != 0,
            want_split: flags & 0b00100000 != 0,
            want_merge: flags & 0b00010000 != 0,
            nx_cc_updated: flags & 0b00001000 != 0,
            next_catchain_seqno,
            next_validator_shard,
            min_ref_mc_seqno,
            gen_utime,
            split_merge_at,
            fees_collected,
            funds_created,
        })
    }
}
