use bitcoin::BlockHash;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum StateAt {
    Latest,
    Block(BlockHash),
}

impl Default for StateAt {
    fn default() -> Self {
        Self::Latest
    }
}

impl StateAt {
    #[inline]
    pub fn to_option(self) -> Option<BlockHash> {
        match self {
            Self::Latest => None,
            Self::Block(blockhash) => Some(blockhash),
        }
    }

    #[inline]
    pub fn resolve(self, fallback: Option<BlockHash>) -> Option<BlockHash> {
        match self {
            Self::Latest => fallback,
            Self::Block(blockhash) => Some(blockhash),
        }
    }
}
