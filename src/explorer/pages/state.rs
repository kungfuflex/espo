use crate::config::get_network;
use crate::modules::essentials::storage::EssentialsProvider;
use crate::runtime::mdb::Mdb;
use bitcoin::Network;
use std::sync::Arc;

#[derive(Clone)]
pub struct ExplorerState {
    pub essentials_mdb: Mdb,
    pub runes_mdb: Mdb,
    pub network: Network,
}

impl ExplorerState {
    pub fn new() -> Self {
        let essentials_mdb = Mdb::from_db(crate::config::get_espo_db(), b"essentials:");
        let runes_mdb = Mdb::from_db(crate::config::get_espo_db(), b"runes:");
        let network = get_network();
        Self { essentials_mdb, runes_mdb, network }
    }

    pub fn essentials_provider(&self) -> EssentialsProvider {
        EssentialsProvider::new(Arc::new(self.essentials_mdb.clone()))
    }
}
