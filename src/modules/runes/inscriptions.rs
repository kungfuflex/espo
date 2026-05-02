use bitcoin::blockdata::{
    opcodes,
    script::{
        Instruction::{self, Op, PushBytes},
        Instructions,
    },
};
use bitcoin::hashes::Hash;
use bitcoin::{Script, Transaction, Txid};
use borsh::{BorshDeserialize, BorshSerialize};
use std::collections::BTreeMap;
use std::iter::Peekable;

const PROTOCOL_ID: &[u8] = b"ord";
const BODY_TAG: &[u8] = &[];
const CONTENT_TYPE_TAG: &[u8] = &[1];
const CONTENT_ENCODING_TAG: &[u8] = &[9];
const DELEGATE_TAG: &[u8] = &[11];

#[derive(BorshSerialize, BorshDeserialize, Clone, Debug, PartialEq, Eq)]
pub struct RuneIcon {
    pub content_type: String,
    pub content_encoding: Option<String>,
    pub body: Vec<u8>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InscriptionRef {
    pub txid: Txid,
    pub index: u32,
}

pub fn image_inscription_from_tx(tx: &Transaction) -> Option<RuneIcon> {
    parsed_inscriptions_from_tx(tx)
        .into_iter()
        .filter_map(ParsedInscription::into_icon)
        .next()
}

pub fn image_inscription_from_tx_at(tx: &Transaction, index: u32) -> Option<RuneIcon> {
    parsed_inscriptions_from_tx(tx)
        .into_iter()
        .nth(index.try_into().ok()?)
        .and_then(ParsedInscription::into_icon)
}

pub fn delegate_inscription_from_tx(tx: &Transaction) -> Option<InscriptionRef> {
    parsed_inscriptions_from_tx(tx)
        .into_iter()
        .find_map(|inscription| inscription.delegate)
}

struct ParsedInscription {
    content_type: Option<String>,
    content_encoding: Option<String>,
    delegate: Option<InscriptionRef>,
    body: Option<Vec<u8>>,
}

impl ParsedInscription {
    fn from_payload(payload: Vec<Vec<u8>>) -> Self {
        let body_index = payload
            .iter()
            .enumerate()
            .position(|(i, push)| i % 2 == 0 && push.as_slice() == BODY_TAG);

        let mut fields: BTreeMap<&[u8], Vec<&[u8]>> = BTreeMap::new();
        for item in payload[..body_index.unwrap_or(payload.len())].chunks(2) {
            if let [key, value] = item {
                fields.entry(key).or_default().push(value);
            }
        }

        let content_type = take_first(&mut fields, CONTENT_TYPE_TAG)
            .and_then(|bytes| std::str::from_utf8(&bytes).ok().map(|s| s.trim().to_string()));
        let content_encoding = take_first(&mut fields, CONTENT_ENCODING_TAG)
            .and_then(|bytes| std::str::from_utf8(&bytes).ok().map(|s| s.trim().to_string()))
            .filter(|s| !s.is_empty());
        let delegate = take_first(&mut fields, DELEGATE_TAG)
            .and_then(|bytes| InscriptionRef::from_value(&bytes));
        let body = body_index.map(|i| payload[i + 1..].iter().flatten().copied().collect());

        Self { content_type, content_encoding, delegate, body }
    }

    fn into_icon(self) -> Option<RuneIcon> {
        let content_type = self.content_type?;
        let content_type_base =
            content_type.split(';').next().unwrap_or_default().trim().to_ascii_lowercase();
        if !matches!(
            content_type_base.as_str(),
            "image/png" | "image/jpeg" | "image/jpg" | "image/webp"
        ) {
            return None;
        }

        let body = self.body?;
        if body.is_empty() {
            return None;
        }

        Some(RuneIcon {
            content_type: if content_type_base == "image/jpg" {
                "image/jpeg".to_string()
            } else {
                content_type
            },
            content_encoding: self.content_encoding,
            body,
        })
    }
}

impl InscriptionRef {
    fn from_value(value: &[u8]) -> Option<Self> {
        if value.len() < Txid::LEN || value.len() > Txid::LEN + 4 {
            return None;
        }

        let (txid, index) = value.split_at(Txid::LEN);
        if let Some(last) = index.last() {
            if index.len() != 4 && *last == 0 {
                return None;
            }
        }

        let index = [
            index.first().copied().unwrap_or_default(),
            index.get(1).copied().unwrap_or_default(),
            index.get(2).copied().unwrap_or_default(),
            index.get(3).copied().unwrap_or_default(),
        ];

        Some(Self { txid: Txid::from_slice(txid).ok()?, index: u32::from_le_bytes(index) })
    }
}

fn parsed_inscriptions_from_tx(tx: &Transaction) -> Vec<ParsedInscription> {
    raw_envelopes_from_tx(tx)
        .into_iter()
        .map(ParsedInscription::from_payload)
        .collect()
}

fn take_first(fields: &mut BTreeMap<&[u8], Vec<&[u8]>>, tag: &[u8]) -> Option<Vec<u8>> {
    let values = fields.get_mut(tag)?;
    if values.is_empty() { None } else { Some(values.remove(0).to_vec()) }
}

fn raw_envelopes_from_tx(tx: &Transaction) -> Vec<Vec<Vec<u8>>> {
    let mut envelopes = Vec::new();
    for input in &tx.input {
        #[allow(deprecated)]
        let Some(tapscript) = input.witness.tapscript() else {
            continue;
        };
        if let Ok(mut found) = raw_envelopes_from_tapscript(tapscript) {
            envelopes.append(&mut found);
        }
    }
    envelopes
}

fn raw_envelopes_from_tapscript(
    tapscript: &Script,
) -> Result<Vec<Vec<Vec<u8>>>, bitcoin::script::Error> {
    let mut envelopes = Vec::new();
    let mut instructions = tapscript.instructions().peekable();

    while let Some(instruction) = instructions.next().transpose()? {
        if is_false_push(&instruction) {
            if let Some(envelope) = envelope_after_false(&mut instructions)? {
                envelopes.push(envelope);
            }
        }
    }

    Ok(envelopes)
}

fn envelope_after_false(
    instructions: &mut Peekable<Instructions<'_>>,
) -> Result<Option<Vec<Vec<u8>>>, bitcoin::script::Error> {
    if !accept_op(instructions, opcodes::all::OP_IF)? {
        return Ok(None);
    }

    match instructions.next().transpose()? {
        Some(PushBytes(push)) if push.as_bytes() == PROTOCOL_ID => {}
        _ => return Ok(None),
    }

    let mut payload = Vec::new();
    loop {
        match instructions.next().transpose()? {
            Some(Op(opcodes::all::OP_ENDIF)) => return Ok(Some(payload)),
            Some(PushBytes(push)) => payload.push(push.as_bytes().to_vec()),
            Some(Op(op)) => {
                let Some(push) = pushnum_payload(op) else {
                    return Ok(None);
                };
                payload.push(push);
            }
            None => return Ok(None),
        }
    }
}

fn accept_op(
    instructions: &mut Peekable<Instructions<'_>>,
    opcode: bitcoin::Opcode,
) -> Result<bool, bitcoin::script::Error> {
    if instructions.peek() == Some(&Ok(Op(opcode))) {
        instructions.next().transpose()?;
        Ok(true)
    } else {
        Ok(false)
    }
}

fn is_false_push(instruction: &Instruction<'_>) -> bool {
    matches!(instruction, PushBytes(push) if push.as_bytes().is_empty())
}

fn pushnum_payload(opcode: bitcoin::Opcode) -> Option<Vec<u8>> {
    if opcode == opcodes::all::OP_PUSHNUM_NEG1 {
        return Some(vec![0x81]);
    }
    let code = opcode.to_u8();
    let first = opcodes::all::OP_PUSHNUM_1.to_u8();
    let last = opcodes::all::OP_PUSHNUM_16.to_u8();
    if (first..=last).contains(&code) { Some(vec![code - first + 1]) } else { None }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bitcoin::blockdata::script::Builder;
    use bitcoin::consensus::deserialize;
    use bitcoin::script::PushBytes;
    use bitcoin::transaction::Version;
    use bitcoin::{OutPoint, ScriptBuf, Sequence, TxIn, TxOut, Witness, absolute::LockTime};

    #[test]
    fn extracts_image_body_from_ord_envelope() {
        let mut builder =
            Builder::new().push_opcode(opcodes::OP_FALSE).push_opcode(opcodes::all::OP_IF);
        builder = push_slice(builder, PROTOCOL_ID);
        builder = push_slice(builder, CONTENT_TYPE_TAG);
        builder = push_slice(builder, b"image/png");
        builder = push_slice(builder, BODY_TAG);
        builder = push_slice(builder, b"png-bytes");
        let script = builder.push_opcode(opcodes::all::OP_ENDIF).into_script();
        let tx = tx_with_witness(Witness::from_slice(&[script.into_bytes(), Vec::new()]));

        let icon = image_inscription_from_tx(&tx).unwrap();

        assert_eq!(icon.content_type, "image/png");
        assert_eq!(icon.content_encoding, None);
        assert_eq!(icon.body, b"png-bytes");
    }

    #[test]
    fn ignores_non_image_envelopes() {
        let mut builder =
            Builder::new().push_opcode(opcodes::OP_FALSE).push_opcode(opcodes::all::OP_IF);
        builder = push_slice(builder, PROTOCOL_ID);
        builder = push_slice(builder, CONTENT_TYPE_TAG);
        builder = push_slice(builder, b"text/plain");
        builder = push_slice(builder, BODY_TAG);
        builder = push_slice(builder, b"hello");
        let script = builder.push_opcode(opcodes::all::OP_ENDIF).into_script();
        let tx = tx_with_witness(Witness::from_slice(&[script.into_bytes(), Vec::new()]));

        assert!(image_inscription_from_tx(&tx).is_none());
    }

    #[test]
    fn extracts_delegate_from_doggotothemoon_etching_tx() {
        let raw = hex::decode(
            "020000000001016be8ccd7100767e73fc1d207dc8993357573f80abbbf14d307fe7c43391d6cfa000000000005000000034a0100000000000022512059bb8e559e5c0f2688ba1f157e4ca2076625bec23d02db529523c7371754ce941027000000000000225120191f3cbc89a06c804202d917b05822857a9fdc675a42b1160468077364c8e4b00000000000000000246a5d2102010487a1c3f0c0ebf7fb9d01010503d4040595e80706808084fea6dee11116010340e21839ce81dcd45e1e6b04df0e6c44d719134a5313da81c2f0fabb5bf46fbc26ab4b1c5110e001c862f496462cad5694a34eaf3478bde460ae9c982ff87820fa5a20658204e3f80250f45924140b103ea13c6ae9e3186f49af92027453a6fa7b1113ac0063036f7264010b205f83846783d4a3a733b68c4f5e77fd4d421c7a18417856c76695e1234efb8f61010200010d0887d0100e5cdff79d6821c0658204e3f80250f45924140b103ea13c6ae9e3186f49af92027453a6fa7b111300000000",
        )
        .unwrap();
        let tx: Transaction = deserialize(&raw).unwrap();

        let delegate = delegate_inscription_from_tx(&tx).unwrap();

        assert_eq!(
            delegate.txid.to_string(),
            "618ffb4e23e19566c7567841187a1c424dfd775e4f8cb633a7a3d4836784835f"
        );
        assert_eq!(delegate.index, 0);
    }

    fn push_slice(builder: Builder, bytes: &[u8]) -> Builder {
        builder.push_slice::<&PushBytes>(bytes.try_into().unwrap())
    }

    fn tx_with_witness(witness: Witness) -> Transaction {
        Transaction {
            version: Version(2),
            lock_time: LockTime::ZERO,
            input: vec![TxIn {
                previous_output: OutPoint::null(),
                script_sig: ScriptBuf::new(),
                sequence: Sequence::ENABLE_RBF_NO_LOCKTIME,
                witness,
            }],
            output: vec![TxOut {
                value: bitcoin::Amount::from_sat(0),
                script_pubkey: ScriptBuf::new(),
            }],
        }
    }
}
