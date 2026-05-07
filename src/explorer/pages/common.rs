use bitcoin::Amount;

pub const ALKANE_SCALE: u128 = 100_000_000;

pub fn fmt_sats(sats: u64) -> String {
    const SATS_PER_BTC: u64 = 100_000_000;

    let whole = sats / SATS_PER_BTC;
    let frac = sats % SATS_PER_BTC;
    if frac == 0 {
        return format!("{whole} BTC");
    }

    let frac = trim_fraction(format!("{frac:08}"));
    format!("{whole}.{frac} BTC")
}

pub fn fmt_amount(amount: Amount) -> String {
    fmt_sats(amount.to_sat())
}

pub fn fmt_alkane_amount(raw: u128) -> String {
    fmt_scaled_amount(raw, 8)
}

pub fn format_integer(n: u128) -> String {
    let mut s = n.to_string();
    let mut i = s.len() as isize - 3;
    while i > 0 {
        s.insert(i as usize, ',');
        i -= 3;
    }
    s
}

pub fn fmt_scaled_amount(raw: u128, decimals: u8) -> String {
    if decimals == 0 {
        return format_integer(raw);
    }

    let scale = 10u128.saturating_pow(decimals as u32);
    let whole = raw / scale;
    let frac = raw % scale;
    if frac == 0 {
        return format_integer(whole);
    }

    let frac = trim_fraction(format!("{:0width$}", frac, width = decimals as usize));
    format!("{}.{}", format_integer(whole), frac)
}

pub fn format_fee_rate_value(rate: f64) -> String {
    let s = format!("{rate:.2}");
    if s == "-0.00" { "0.00".to_string() } else { s }
}

pub fn format_fee_rate(rate: f64) -> String {
    format!("{} sat/vB", format_fee_rate_value(rate))
}

fn trim_fraction(mut s: String) -> String {
    while s.ends_with('0') {
        s.pop();
    }
    s
}
