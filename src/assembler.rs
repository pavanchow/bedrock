//! A two-pass assembler for Bedrock assembly text.
//!
//! Syntax:
//! - Comments start with `;` and run to end of line.
//! - Labels are `name:` and may sit on their own line or before an instruction.
//! - Directives: `.org ADDR`, `.word V, ...`, `.byte V, ...`, `.string "..."`,
//!   `.space N`, `.equ NAME VALUE`.
//! - Registers are `r0`..`r7` (case-insensitive).
//! - Memory operands are `[rb]`, `[rb + imm]`, or `[rb - imm]`.
//! - Immediates are decimal, `0x` hex, a `'c'` char, a label, or an `.equ`
//!   constant, each optionally followed by `+ N` / `- N` terms.

use std::collections::HashMap;

use crate::isa::*;

/// The result of assembling a program.
#[derive(Debug, Clone)]
pub struct Assembled {
    /// Lowest address written (from the first `.org`, default 0).
    pub origin: u32,
    /// The machine-code image starting at `origin`.
    pub code: Vec<u8>,
    /// Resolved label addresses.
    pub labels: HashMap<String, u32>,
}

struct Parsed {
    line_no: usize,
    addr: u32,
    kind: Kind,
}

enum Kind {
    Instr {
        op: Op,
         operands: Vec<String>,
    },
    Word(Vec<String>),
    Byte(Vec<String>),
    Str(Vec<u8>),
    Space,
}

/// Assemble source text into a machine-code image.
pub fn assemble(src: &str) -> Result<Assembled, String> {
    let mut labels: HashMap<String, u32> = HashMap::new();
    let mut consts: HashMap<String, u32> = HashMap::new();
    let mut parsed: Vec<Parsed> = Vec::new();
    let mut addr: u32 = 0;

    // Pass 1: assign addresses, collect labels and constants.
    for (idx, raw) in src.lines().enumerate() {
        let line_no = idx + 1;
        let line = strip_comment(raw).trim();
        if line.is_empty() {
            continue;
        }

        let mut rest = line;

        // Leading labels, possibly several: `a: b: instr`.
        loop {
            if let Some(colon) = rest.find(':') {
                let name = rest[..colon].trim();
                if is_ident(name) && !rest[..colon].contains(char::is_whitespace) {
                    if labels.insert(name.to_string(), addr).is_some() {
                        return Err(format!("line {line_no}: duplicate label '{name}'"));
                    }
                    rest = rest[colon + 1..].trim();
                    if rest.is_empty() {
                        break;
                    }
                    continue;
                }
            }
            break;
        }
        if rest.is_empty() {
            continue;
        }

        let mut toks = tokenize(rest);
        let head = toks.remove(0).to_lowercase();

        if let Some(dir) = head.strip_prefix('.') {
            match dir {
                "org" => {
                    addr = eval_now(&toks.join(""), &consts, line_no)?;
                }
                "equ" => {
                    if toks.len() < 2 {
                        return Err(format!("line {line_no}: .equ needs NAME VALUE"));
                    }
                    let name = toks[0].clone();
                    let v = eval_now(&toks[1..].join(""), &consts, line_no)?;
                    consts.insert(name, v);
                }
                "word" => {
                    let items = split_commas(&toks.join(" "));
                    let n = items.len() as u32;
                    parsed.push(Parsed {
                        line_no,
                        addr,
                        kind: Kind::Word(items),
                    });
                    addr += 4 * n;
                }
                "byte" => {
                    let items = split_commas(&toks.join(" "));
                    let n = items.len() as u32;
                    parsed.push(Parsed {
                        line_no,
                        addr,
                        kind: Kind::Byte(items),
                    });
                    addr += n;
                }
                "string" => {
                    let bytes = parse_string(rest, line_no)?;
                    let n = bytes.len() as u32;
                    parsed.push(Parsed {
                        line_no,
                        addr,
                        kind: Kind::Str(bytes),
                    });
                    addr += n;
                }
                "space" => {
                    let n = eval_now(&toks.join(""), &consts, line_no)?;
                    parsed.push(Parsed {
                        line_no,
                        addr,
                        kind: Kind::Space,
                    });
                    addr += n;
                }
                other => return Err(format!("line {line_no}: unknown directive .{other}")),
            }
            continue;
        }

        let op = Op::from_mnemonic(&head)
            .ok_or_else(|| format!("line {line_no}: unknown mnemonic '{head}'"))?;
        let operands = split_commas(&toks.join(" "));
        parsed.push(Parsed {
            line_no,
            addr,
            kind: Kind::Instr { op, operands },
        });
        addr += INSTR_SIZE;
    }

    let origin = parsed.first().map(|p| p.addr).unwrap_or(0);
    let end = addr;
    if end < origin {
        return Err("assembled image ends before its origin".to_string());
    }
    let mut code = vec![0u8; (end - origin) as usize];

    // Pass 2: emit bytes with all labels resolved.
    let resolve = |tok: &str, line_no: usize| -> Result<u32, String> {
        eval_expr(tok, &labels, &consts, line_no)
    };

    for p in &parsed {
        let off = (p.addr - origin) as usize;
        match &p.kind {
            Kind::Instr { op, operands } => {
                let instr = encode_instr(*op, operands, &labels, &consts, p.line_no)?;
                code[off..off + 8].copy_from_slice(&instr.encode());
            }
            Kind::Word(items) => {
                for (i, it) in items.iter().enumerate() {
                    let v = resolve(it, p.line_no)?;
                    code[off + i * 4..off + i * 4 + 4].copy_from_slice(&v.to_le_bytes());
                }
            }
            Kind::Byte(items) => {
                for (i, it) in items.iter().enumerate() {
                    let v = resolve(it, p.line_no)?;
                    code[off + i] = v as u8;
                }
            }
            Kind::Str(bytes) => {
                code[off..off + bytes.len()].copy_from_slice(bytes);
            }
            Kind::Space => {}
        }
    }

    Ok(Assembled {
        origin,
        code,
        labels,
    })
}

fn encode_instr(
    op: Op,
    operands: &[String],
    labels: &HashMap<String, u32>,
    consts: &HashMap<String, u32>,
    line_no: usize,
) -> Result<Instr, String> {
    let want = |n: usize| -> Result<(), String> {
        if operands.len() == n {
            Ok(())
        } else {
            Err(format!(
                "line {line_no}: {} expects {} operands, got {}",
                op.mnemonic(),
                n,
                operands.len()
            ))
        }
    };
    let reg = |s: &str| parse_reg(s).ok_or_else(|| format!("line {line_no}: bad register '{s}'"));
    let imm = |s: &str| eval_expr(s, labels, consts, line_no);

    let mut instr = Instr {
        op,
        a: 0,
        b: 0,
        c: 0,
        imm: 0,
    };

    match op.form() {
        Form::None => want(0)?,
        Form::Rrr => {
            want(3)?;
            instr.a = reg(&operands[0])?;
            instr.b = reg(&operands[1])?;
            instr.c = reg(&operands[2])?;
        }
        Form::Rri => {
            want(3)?;
            instr.a = reg(&operands[0])?;
            instr.b = reg(&operands[1])?;
            instr.imm = imm(&operands[2])?;
        }
        Form::Rr => {
            want(2)?;
            instr.a = reg(&operands[0])?;
            instr.b = reg(&operands[1])?;
        }
        Form::Ri => {
            want(2)?;
            instr.a = reg(&operands[0])?;
            instr.imm = imm(&operands[1])?;
        }
        Form::Rr2 => {
            want(2)?;
            instr.a = reg(&operands[0])?;
            instr.b = reg(&operands[1])?;
        }
        Form::Ri2 => {
            want(2)?;
            instr.a = reg(&operands[0])?;
            instr.imm = imm(&operands[1])?;
        }
        Form::MemLoad | Form::MemStore => {
            want(2)?;
            instr.a = reg(&operands[0])?;
            let (rb, disp) = parse_mem(&operands[1], labels, consts, line_no)?;
            instr.b = rb;
            instr.imm = disp;
        }
        Form::Reg => {
            want(1)?;
            instr.a = reg(&operands[0])?;
        }
        Form::Imm => {
            want(1)?;
            instr.imm = imm(&operands[0])?;
        }
    }
    Ok(instr)
}

// ---- lexing helpers ----------------------------------------------------

fn strip_comment(line: &str) -> &str {
    match line.find(';') {
        Some(i) => &line[..i],
        None => line,
    }
}

fn is_ident(s: &str) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' || c == '.' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.')
}

/// Split into whitespace tokens, keeping bracketed groups and quoted strings
/// intact and treating commas as separators.
fn tokenize(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_str = false;
    let mut in_brk = false;
    for ch in s.chars() {
        match ch {
            '"' => {
                in_str = !in_str;
                cur.push(ch);
            }
            '[' if !in_str => {
                in_brk = true;
                cur.push(ch);
            }
            ']' if !in_str => {
                in_brk = false;
                cur.push(ch);
            }
            c if c.is_whitespace() && !in_str && !in_brk => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            _ => cur.push(ch),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// Split an operand list on commas that are not inside brackets or quotes.
fn split_commas(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_str = false;
    let mut depth = 0;
    for ch in s.chars() {
        match ch {
            '"' => {
                in_str = !in_str;
                cur.push(ch);
            }
            '[' if !in_str => {
                depth += 1;
                cur.push(ch);
            }
            ']' if !in_str => {
                depth -= 1;
                cur.push(ch);
            }
            ',' if !in_str && depth == 0 => {
                let t = cur.trim().to_string();
                if !t.is_empty() {
                    out.push(t);
                }
                cur.clear();
            }
            _ => cur.push(ch),
        }
    }
    let t = cur.trim().to_string();
    if !t.is_empty() {
        out.push(t);
    }
    out
}

fn parse_string(line: &str, line_no: usize) -> Result<Vec<u8>, String> {
    let start = line
        .find('"')
        .ok_or_else(|| format!("line {line_no}: .string needs a quoted literal"))?;
    let end = line
        .rfind('"')
        .filter(|&e| e > start)
        .ok_or_else(|| format!("line {line_no}: unterminated string"))?;
    let body = &line[start + 1..end];
    let mut out = Vec::new();
    let mut chars = body.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n') => out.push(b'\n'),
                Some('t') => out.push(b'\t'),
                Some('0') => out.push(0),
                Some('\\') => out.push(b'\\'),
                Some('"') => out.push(b'"'),
                Some(other) => out.push(other as u8),
                None => return Err(format!("line {line_no}: dangling escape")),
            }
        } else {
            out.push(c as u8);
        }
    }
    Ok(out)
}

// ---- value parsing -----------------------------------------------------

fn parse_reg(s: &str) -> Option<u8> {
    let s = s.trim().to_lowercase();
    let n = s.strip_prefix('r')?;
    let idx: u8 = n.parse().ok()?;
    if (idx as usize) < NUM_REGS {
        Some(idx)
    } else {
        None
    }
}

fn parse_mem(
    s: &str,
    labels: &HashMap<String, u32>,
    consts: &HashMap<String, u32>,
    line_no: usize,
) -> Result<(u8, u32), String> {
    let inner = s
        .trim()
        .strip_prefix('[')
        .and_then(|x| x.strip_suffix(']'))
        .ok_or_else(|| format!("line {line_no}: memory operand must be [..]"))?;
    // Forms: rb | rb + expr | rb - expr
    if let Some(pos) = inner.find(['+', '-']) {
        let rb = parse_reg(inner[..pos].trim())
            .ok_or_else(|| format!("line {line_no}: bad base register in '{s}'"))?;
        let sign = &inner[pos..pos + 1];
        let rest = inner[pos + 1..].trim();
        let mag = eval_expr(rest, labels, consts, line_no)?;
        let disp = if sign == "-" { 0u32.wrapping_sub(mag) } else { mag };
        Ok((rb, disp))
    } else {
        let rb = parse_reg(inner.trim())
            .ok_or_else(|| format!("line {line_no}: bad base register in '{s}'"))?;
        Ok((rb, 0))
    }
}

/// Evaluate an expression with labels available.
fn eval_expr(
    s: &str,
    labels: &HashMap<String, u32>,
    consts: &HashMap<String, u32>,
    line_no: usize,
) -> Result<u32, String> {
    let s = s.trim();
    // Split into additive terms while keeping their signs.
    let mut acc: u32 = 0;
    let mut sign: i64 = 1;
    let mut term = String::new();
    let mut first = true;
    let flush = |term: &str,
                 sign: i64,
                 acc: &mut u32,
                 first: bool|
     -> Result<(), String> {
        let t = term.trim();
        if t.is_empty() {
            if first {
                return Ok(());
            }
            return Err(format!("line {line_no}: empty term in '{s}'"));
        }
        let v = eval_term(t, labels, consts, line_no)?;
        if sign >= 0 {
            *acc = acc.wrapping_add(v);
        } else {
            *acc = acc.wrapping_sub(v);
        }
        Ok(())
    };
    for ch in s.chars() {
        match ch {
            '+' | '-' if !term.trim().is_empty() => {
                flush(&term, sign, &mut acc, first)?;
                first = false;
                term.clear();
                sign = if ch == '-' { -1 } else { 1 };
            }
            _ => term.push(ch),
        }
    }
    flush(&term, sign, &mut acc, first)?;
    Ok(acc)
}

fn eval_term(
    t: &str,
    labels: &HashMap<String, u32>,
    consts: &HashMap<String, u32>,
    line_no: usize,
) -> Result<u32, String> {
    if let Some(v) = parse_num(t) {
        return Ok(v);
    }
    if let Some(v) = parse_char(t) {
        return Ok(v);
    }
    if let Some(v) = consts.get(t) {
        return Ok(*v);
    }
    if let Some(v) = labels.get(t) {
        return Ok(*v);
    }
    Err(format!("line {line_no}: cannot resolve '{t}'"))
}

/// Evaluate an expression with only constants available (pass 1 directives).
fn eval_now(s: &str, consts: &HashMap<String, u32>, line_no: usize) -> Result<u32, String> {
    let empty = HashMap::new();
    eval_expr(s, &empty, consts, line_no)
}

fn parse_num(s: &str) -> Option<u32> {
    let s = s.trim();
    if let Some(hex) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        return u32::from_str_radix(hex, 16).ok();
    }
    if let Some(bin) = s.strip_prefix("0b") {
        return u32::from_str_radix(bin, 2).ok();
    }
    if let Some(neg) = s.strip_prefix('-') {
        return neg.parse::<i64>().ok().map(|v| (-v) as u32);
    }
    s.parse::<u32>().ok()
}

fn parse_char(s: &str) -> Option<u32> {
    let s = s.trim();
    let bytes = s.as_bytes();
    if bytes.len() >= 3 && bytes[0] == b'\'' && bytes[bytes.len() - 1] == b'\'' {
        let body = &s[1..s.len() - 1];
        if body == "\\n" {
            return Some(b'\n' as u32);
        }
        if body == "\\t" {
            return Some(b'\t' as u32);
        }
        if body == "\\0" {
            return Some(0);
        }
        let mut it = body.chars();
        let c = it.next()?;
        if it.next().is_none() {
            return Some(c as u32);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_operands_respecting_brackets() {
        let toks = split_commas("r0, [r1 + 4], r2");
        assert_eq!(toks, vec!["r0", "[r1 + 4]", "r2"]);
    }

    #[test]
    fn numbers_in_several_bases() {
        assert_eq!(parse_num("42"), Some(42));
        assert_eq!(parse_num("0x2a"), Some(42));
        assert_eq!(parse_num("0b101010"), Some(42));
        assert_eq!(parse_num("-1"), Some(0xFFFF_FFFF));
    }

    #[test]
    fn char_literals() {
        assert_eq!(parse_char("'A'"), Some(65));
        assert_eq!(parse_char("'\\n'"), Some(10));
    }

    #[test]
    fn expression_with_offset() {
        let mut labels = HashMap::new();
        labels.insert("base".to_string(), 0x100);
        let v = eval_expr("base + 8", &labels, &HashMap::new(), 1).unwrap();
        assert_eq!(v, 0x108);
    }

    #[test]
    fn comment_stripping() {
        assert_eq!(strip_comment("mov r0, r1 ; hi").trim(), "mov r0, r1");
    }

    #[test]
    fn rejects_unknown_mnemonic() {
        assert!(assemble("frobnicate r0").is_err());
    }
}
