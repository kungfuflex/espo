use anyhow::Result;

pub trait PriceFeed {
    fn get_bitcoin_price_usd_at_block_height(&self, height: u64) -> Result<u128>;
}
