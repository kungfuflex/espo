use crate::schemas::SchemaAlkaneId;
use bitcoin::Network;

pub const KEY_INDEX_HEIGHT: &[u8] = b"/index_height";

pub fn get_subfrost_wrap_address(network: Network) -> &'static str {
    match network {
        Network::Bitcoin => "bc1p5lushqjk7kxpqa87ppwn0dealucyqa6t40ppdkhpqm3grcpqvw9s3wdsx7",
        _ => "",
    }
}

pub fn get_frbtc_alkane(network: Network) -> SchemaAlkaneId {
    match network {
        Network::Bitcoin => SchemaAlkaneId { block: 32, tx: 0 },
        _ => SchemaAlkaneId { block: 32, tx: 0 },
    }
}
