pub const DEFAULT_PAGE_LIMIT: usize = 25;
pub const MAX_PAGE_LIMIT: usize = 200;

use bitcoin::Network;

use crate::config::get_network;

pub const ALKANE_TOKEN_ICON_BASE: &str = "https://cdn.ordiscan.com/alkanes";
pub const ALKANE_CONTRACT_ICON_BASE: &str = "https://cdn.ordiscan.com/alkanes";
const FRBTC_ICON_URL: &str = "https://i.ibb.co/6cR2hC05/frbtc-improved-1.png";

// --- Mainnet overrides ---
const MAINNET_ALKANE_NAME_OVERRIDES: &[(&str, &str, &str)] =
    &[("2:0", "DIESEL", "DIESEL"), ("32:0", "frBTC", "FRBTC"), ("2:68479", "TORTILLA", "TORTILLA")];
const MAINNET_ICON_OVERRIDES: &[(&str, &str)] = &[
    ("2:68479", "https://cdn.idclub.io/alkanes/2-62083.webp"),
    ("2:77269", "https://i.ibb.co/RTZw3zyh/tortilla-Lp-2.png"),
    ("2:77623", "https://i.ibb.co/nN1LKyZb/fire.png"),
    ("32:0", FRBTC_ICON_URL),
];
// Protocol infrastructure, named. These contracts are not tokens, so the token
// metadata index has nothing to say about them and the explorer would otherwise
// render a bare "4:65522" on every call, trace frame and holder row.
//
// Naming a contract is NOT vouching for it: impersonation is decided by the
// canonical set, not here.
//
// Every id is taken from a config production uses, not from memory:
//   [app]  subfrost-app utils/getConfig.ts FIRE_MAINNET_CONTRACTS
//   [alk]  alkanes-support src/constants.rs AUTH_TOKEN_FACTORY_ID (0xffed)
// The AMM implementation and beacon behind the factory proxy are deliberately
// absent: no config here can cite them, and a wrong id mislabels a contract.
// Mirrored in apps/explorer/lib/alkanes/contracts.ts; change both together.
const MAINNET_CONTRACT_NAME_OVERRIDES: &[(&str, &str)] = &[
    ("4:65522", "Oyl AMM Factory"),          // [app] ALKANE_FACTORY_ID
    ("4:65517", "Auth Token Factory"),       // [alk]
    ("2:77087", "DIESEL/frBTC LP"),          // [app] FIRE_LP_TOKEN_ID
    ("2:77623", "FIRE"),                     // [app] FIRE_TOKEN_ID
    ("2:77624", "FIRE Staking Factory"),     // [app] FIRE_STAKING_FACTORY_ID
    ("2:77625", "FIRE Redemption"),          // [app] FIRE_REDEMPTION_ID
    ("2:77626", "FIRE Price Oracle"),        // [app] FIRE_PRICE_ORACLE_ID
    ("2:77627", "FIRE Bonding"),             // [app] FIRE_BONDING_ID
    ("2:77628", "FIRE Treasury"),            // [app] FIRE_TREASURY_ID
    ("2:77621", "FIRE Staking Beacon"),      // [app] FIRE_STAKING_BEACON_ID
    ("2:77622", "FIRE Position Beacon"),     // [app] FIRE_POSITION_BEACON_ID
    ("2:77631", "FIRE Epoch-0 Staking"),     // [app] FIRE_EPOCH_0_STAKING_ID
    ("2:70003", "DIESEL Claim Distributor"), // [app] DIESEL_CLAIM_MERKLE_DISTRIBUTOR_ID
    ("4:47876", "PSBT Lending Settlement Template"), // [app] LENDING_PSBT_TEMPLATE_ID
    ("4:6666", "Zippo Zap + Bond Router"),   // [app] ZIPPO_ID
    ("4:76", "bUSD Splitter"),               // [app] BUSD_SPLITTER_ID
];
const MAINNET_FACTORY_ICON_BLACKLIST: &[&str] =
    &["4:3804", "4:103", "4:102", "4:3803", "4:3805", "4:3806", "4:3807", "4:3800", "4:3802"];

// --- Regtest overrides (extend as needed) ---
const REGTEST_ALKANE_NAME_OVERRIDES: &[(&str, &str, &str)] = &[];
const REGTEST_ICON_OVERRIDES: &[(&str, &str)] = &[("32:0", FRBTC_ICON_URL)];
const REGTEST_CONTRACT_NAME_OVERRIDES: &[(&str, &str)] = &[("4:65522", "Oyl AMM")];
const REGTEST_FACTORY_ICON_BLACKLIST: &[&str] = &[];

pub fn alkane_name_overrides() -> &'static [(&'static str, &'static str, &'static str)] {
    match get_network() {
        Network::Bitcoin => MAINNET_ALKANE_NAME_OVERRIDES,
        Network::Regtest => REGTEST_ALKANE_NAME_OVERRIDES,
        _ => MAINNET_ALKANE_NAME_OVERRIDES,
    }
}

pub fn alkane_icon_overrides() -> &'static [(&'static str, &'static str)] {
    match get_network() {
        Network::Bitcoin => MAINNET_ICON_OVERRIDES,
        Network::Regtest => REGTEST_ICON_OVERRIDES,
        _ => MAINNET_ICON_OVERRIDES,
    }
}

/// Optional overrides specifically for contract display names.
pub fn alkane_contract_name_overrides() -> &'static [(&'static str, &'static str)] {
    match get_network() {
        Network::Bitcoin => MAINNET_CONTRACT_NAME_OVERRIDES,
        Network::Regtest => REGTEST_CONTRACT_NAME_OVERRIDES,
        _ => MAINNET_CONTRACT_NAME_OVERRIDES,
    }
}

pub fn alkane_factory_icon_blacklist() -> &'static [&'static str] {
    match get_network() {
        Network::Bitcoin => MAINNET_FACTORY_ICON_BLACKLIST,
        Network::Regtest => REGTEST_FACTORY_ICON_BLACKLIST,
        _ => MAINNET_FACTORY_ICON_BLACKLIST,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        FRBTC_ICON_URL, MAINNET_CONTRACT_NAME_OVERRIDES, MAINNET_ICON_OVERRIDES,
        REGTEST_CONTRACT_NAME_OVERRIDES, REGTEST_ICON_OVERRIDES,
    };

    #[test]
    fn frbtc_icon_is_overridden_on_mainnet_and_regtest() {
        for overrides in [MAINNET_ICON_OVERRIDES, REGTEST_ICON_OVERRIDES] {
            assert!(overrides.contains(&("32:0", FRBTC_ICON_URL)));
        }
    }

    #[test]
    fn oyl_amm_factory_is_named_on_mainnet_and_regtest() {
        for overrides in [MAINNET_CONTRACT_NAME_OVERRIDES, REGTEST_CONTRACT_NAME_OVERRIDES] {
            assert!(
                overrides.iter().any(|(id, name)| *id == "4:65522" && name.contains("Oyl AMM"))
            );
        }
    }

    /// Two contracts sharing an id would make one of them render under the
    /// other's name, which is worse than rendering a bare id.
    #[test]
    fn contract_name_overrides_have_no_duplicate_ids() {
        for overrides in [MAINNET_CONTRACT_NAME_OVERRIDES, REGTEST_CONTRACT_NAME_OVERRIDES] {
            let mut ids: Vec<&str> = overrides.iter().map(|(id, _)| *id).collect();
            ids.sort_unstable();
            let before = ids.len();
            ids.dedup();
            assert_eq!(ids.len(), before, "duplicate id in contract name overrides");
        }
    }

    /// The FIRE suite is the set most often read as bare ids in a trace.
    #[test]
    fn fire_infrastructure_is_named_on_mainnet() {
        for id in ["2:77624", "2:77625", "2:77626", "2:77627", "2:77628"] {
            assert!(
                MAINNET_CONTRACT_NAME_OVERRIDES.iter().any(|(known, _)| *known == id),
                "missing FIRE contract {id}",
            );
        }
    }
}
