use tl_proto::{TlRead, TlWrite};

#[derive(Debug, Copy, Clone, Eq, PartialEq, TlRead, TlWrite)]
#[tl(boxed, scheme = "persistent_state.tl")]
pub enum PreparedState {
    #[tl(id = "tonNode.preparedState")]
    Found,
    #[tl(id = "tonNode.notFoundState")]
    NotFound,
}

#[derive(Clone, TlRead, TlWrite)]
#[tl(
    boxed,
    id = "tonNode.preparePersistentState",
    scheme = "persistent_state.tl"
)]
pub struct RpcPreparePersistentState {
    #[tl(with = "tl_block_id")]
    pub block: ton_block::BlockIdExt,
    #[tl(with = "tl_block_id")]
    pub masterchain_block: ton_block::BlockIdExt,
}

#[derive(Clone, TlRead, TlWrite)]
#[tl(
    boxed,
    id = "tonNode.downloadPersistentStateSlice",
    scheme = "persistent_state.tl"
)]
pub struct RpcDownloadPersistentStateSlice {
    #[tl(with = "tl_block_id")]
    pub block: ton_block::BlockIdExt,
    #[tl(with = "tl_block_id")]
    pub masterchain_block: ton_block::BlockIdExt,
    pub offset: u64,
    pub max_size: u64,
}

mod tl_block_id {
    use tl_proto::{TlError, TlPacket, TlRead, TlResult};

    pub const SIZE_HINT: usize = 80;

    pub const fn size_hint(_: &ton_block::BlockIdExt) -> usize {
        SIZE_HINT
    }

    pub fn write<P: TlPacket>(block: &ton_block::BlockIdExt, packet: &mut P) {
        packet.write_i32(block.shard_id.workchain_id());
        packet.write_u64(block.shard_id.shard_prefix_with_tag());
        packet.write_u32(block.seq_no);
        packet.write_raw_slice(block.root_hash.as_slice());
        packet.write_raw_slice(block.file_hash.as_slice());
    }

    pub fn read(packet: &[u8], offset: &mut usize) -> TlResult<ton_block::BlockIdExt> {
        let shard_id = ton_block::ShardIdent::with_tagged_prefix(
            i32::read_from(packet, offset)?,
            u64::read_from(packet, offset)?,
        )
        .map_err(|_| TlError::InvalidData)?;
        let seq_no = u32::read_from(packet, offset)?;
        let root_hash = <[u8; 32]>::read_from(packet, offset)?;
        let file_hash = <[u8; 32]>::read_from(packet, offset)?;

        Ok(ton_block::BlockIdExt {
            shard_id,
            seq_no,
            root_hash: root_hash.into(),
            file_hash: file_hash.into(),
        })
    }
}
