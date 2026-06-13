use crate::schemas::SchemaAlkaneId;

pub struct PhishingAlkaneWarning {
    pub id: SchemaAlkaneId,
    pub note_en: &'static str,
    pub note_zh: &'static str,
}

// Add phishing/scam alkanes here. The id format is block:tx.
//
// Example:
// PhishingAlkaneWarning {
//     id: SchemaAlkaneId { block: 2, tx: 12345 },
//     note_en: "This alkane impersonates a known project. Do not trade or interact with it.",
//     note_zh: "该 Alkane 冒充已知项目。请勿交易或与其交互。",
// },
pub const PHISHING_ALKANE_WARNINGS: &[PhishingAlkaneWarning] = &[PhishingAlkaneWarning {
    id: SchemaAlkaneId { block: 4, tx: 31425 },
    note_en: "This alkane was detected to be a phishing attempt impersonating the pizza.fun project. The project airdropped TORTILLA holders, the community of pizza.fun, to trick their community to think this was pizza.fun's token. Pizza.fun's founder, mork1e, has already stated on X this token is not his: https://x.com/mork1e/status/2065242600521519246?s=20",
    note_zh: "该 Alkane 被检测为冒充 pizza.fun 项目的钓鱼尝试。该项目向 TORTILLA 持有者，即 pizza.fun 社区，进行了空投，试图诱骗社区以为这是 pizza.fun 的代币。Pizza.fun 创始人 mork1e 已经在 X 上声明该代币不是他的：https://x.com/mork1e/status/2065242600521519246?s=20",
}];

pub fn phishing_warning_for(id: &SchemaAlkaneId) -> Option<&'static PhishingAlkaneWarning> {
    PHISHING_ALKANE_WARNINGS.iter().find(|warning| warning.id == *id)
}

pub fn is_phishing_alkane(id: &SchemaAlkaneId) -> bool {
    phishing_warning_for(id).is_some()
}
