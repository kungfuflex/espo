use crate::schemas::SchemaAlkaneId;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AlkaneWarningKind {
    Scam,
    Notice,
}

pub struct PhishingAlkaneWarning {
    pub id: SchemaAlkaneId,
    pub kind: AlkaneWarningKind,
    pub note_en: &'static str,
    pub note_zh: &'static str,
}

impl PhishingAlkaneWarning {
    pub fn is_scam(&self) -> bool {
        self.kind == AlkaneWarningKind::Scam
    }
}

// Add phishing/scam alkanes here. The id format is block:tx.
//
// Example:
// PhishingAlkaneWarning {
//     id: SchemaAlkaneId { block: 2, tx: 12345 },
//     kind: AlkaneWarningKind::Scam,
//     note_en: "This alkane impersonates a known project. Do not trade or interact with it.",
//     note_zh: "该 Alkane 冒充已知项目。请勿交易或与其交互。",
// },
pub const PHISHING_ALKANE_WARNINGS: &[PhishingAlkaneWarning] = &[PhishingAlkaneWarning {
    id: SchemaAlkaneId { block: 4, tx: 31425 },
    kind: AlkaneWarningKind::Notice,
    note_en: "This Alkane should not be confused with pizza.fun's unreleased token and is in no way related to pizza.fun. Please see the official stance on this here: https://x.com/mork1e/status/2065242600521519246?s=20",
    note_zh: "请勿将该 Alkane 与 pizza.fun 尚未发布的代币混淆；它与 pizza.fun 没有任何关系。请在此查看官方立场：https://x.com/mork1e/status/2065242600521519246?s=20",
}];

pub fn phishing_warning_for(id: &SchemaAlkaneId) -> Option<&'static PhishingAlkaneWarning> {
    PHISHING_ALKANE_WARNINGS.iter().find(|warning| warning.id == *id)
}

pub fn is_phishing_alkane(id: &SchemaAlkaneId) -> bool {
    phishing_warning_for(id).map(|warning| warning.is_scam()).unwrap_or(false)
}
