### ShardStateUnsplit (old -> tycho)

- `global_id`, `seq_no`, `vert_seq_no`, `gen_time`, `gen_lt`, `min_ref_mc_seqno`, `before_split` map 1:1.
- `gen_utime_ms = 0`.
- `processed_upto = ShardStateUnsplit::empty_processed_upto_info().clone()`.
- `out_msg_queue_info` is ignored.
- `accounts` must be rebuilt.
- `overload_history`, `underload_history` map 1:1.
- `total_balance`, `total_validator_fees` via BOC bridge (see "Bridge via BOC").
- `libraries`, `master_ref` mapped via BOC bridge / simple field mapping.
- `custom` (masterchain only) rebuilt as `McStateExtra`.

### ShardStateUnsplit differences

| Field                     | ton-labs-block          | tycho-types                   | Migration action                                             |
|---------------------------|-------------------------|-------------------------------|--------------------------------------------------------------|
| TL-B tag                  | `#9023afe2`             | `#9023aeee`                   | Rebuild state; old tag will not load                         |
| `gen_utime`               | `uint32`                | `uint32`                      | Map 1:1                                                      |
| `gen_utime_ms`            | absent (except venom)   | `uint16`                      | Set `0`                                                      |
| `out_msg_queue_info`      | present                 | absent                        | Ignore                                                       |
| `processed_upto`          | absent                  | present (`ProcessedUptoInfo`) | Set empty (`ShardStateUnsplit::empty_processed_upto_info()`) |
| `accounts`                | `ShardAccounts`         | `ShardAccounts`               | Rebuild with strict mapping                                  |
| `total_balance`           | `CurrencyCollection`    | `CurrencyCollection`          | Bridge via BOC                                               |
| `total_validator_fees`    | `CurrencyCollection`    | `CurrencyCollection`          | Bridge via BOC                                               |
| `libraries`               | `HashmapE 256 LibDescr` | `Dict<HashBytes, LibDescr>`   | Bridge via BOC                                               |
| `master_ref`              | `Maybe BlkMasterInfo`   | `Option<BlockRef>`            | Field-by-field mapping                                       |
| `custom` (`McStateExtra`) | different layout        | different layout              | Rebuild (drop copyleft, add consensus_info)                  |

### McStateExtra differences

| Field                               | ton-labs-block   | tycho-types                                          | Migration action                   |
|-------------------------------------|------------------|------------------------------------------------------|------------------------------------|
| Copyleft fields                     | present          | absent                                               | Drop (do not map)                  |
| `consensus_info`                    | absent           | present                                              | Set to `ConsensusInfo::ZEROSTATE`  |
| `ShardDescription` extra fields     | missing          | `ext_processed_to_anchor_id`, `top_sc_block_updated` | Set `0` / `false`                  |
| `ShardDescription.split_merge_at`   | includes `None`  | `Option<FutureSplitMerge>`                           | Map `None -> None`, `Some -> Some` |
| `config`                            | `ConfigParams`   | `BlockchainConfig`                                   | Bridge via BOC                     |
| `validator_info`                    | present          | present                                              | Bridge via BOC                     |
| `prev_blocks`                       | present          | present                                              | Bridge via BOC                     |
| `after_key_block`, `last_key_block` | present          | present                                              | Map 1:1                            |
| `block_create_stats`                | present/optional | present/optional                                     | Bridge via BOC if present          |
| `global_balance`                    | present          | present                                              | Bridge via BOC                     |

### Accounts mapping

Old `ton_block::Account` -> new `tycho_types::models::Account`:

| Old field                         | New field                          | Notes                                     |
|-----------------------------------|------------------------------------|-------------------------------------------|
| `addr: MsgAddressInt`             | `address: IntAddr`                 | bridge via BOC (see below)                |
| `storage_stat.used.cells/bits`    | `storage_stat.used.cells/bits`     | `VarUint56::new`                          |
| `storage_stat.public_cells`       | none                               | must be zero, otherwise fail              |
| `storage_stat.last_paid`          | `storage_stat.last_paid`           | 1:1                                       |
| `storage_stat.due_payment: Grams` | `storage_stat.due_payment: Tokens` | `Tokens::new(grams.as_u128())`            |
| `storage.state`                   | `state`                            | Uninit / Active(StateInit) / Frozen(hash) |
| `storage.balance`                 | `balance`                          | bridge via BOC                            |
| `storage.last_trans_lt`           | `last_trans_lt`                    | 1:1                                       |

Fail on `public_cells != 0` or just skip this field anyway? Since tycho `StorageInfo` has no field for it; strict
equality probably requires not dropping data?