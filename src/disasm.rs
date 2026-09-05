//! Turn machine code back into Bedrock assembly text.

use crate::isa::*;

/// Disassemble a single instruction word into a text line (no trailing newline).
pub fn disasm_instr(instr: Instr) -> String {
    let m = instr.op.mnemonic();
    let a = instr.a & 0x07;
    let b = instr.b & 0x07;
    let c = instr.c & 0x07;
    let imm = instr.imm;
    match instr.op.form() {
        Form::None => m.to_string(),
        Form::Rrr => format!("{m} r{a}, r{b}, r{c}"),
        Form::Rri => format!("{m} r{a}, r{b}, {imm}"),
        Form::Rr => format!("{m} r{a}, r{b}"),
        Form::Ri => format!("{m} r{a}, {imm}"),
        Form::Rr2 => format!("{m} r{a}, r{b}"),
        Form::Ri2 => format!("{m} r{a}, {imm}"),
        Form::MemLoad | Form::MemStore => {
            let d = imm as i32;
            if d == 0 {
                format!("{m} r{a}, [r{b}]")
            } else if d < 0 {
                format!("{m} r{a}, [r{b} - {}]", -(d as i64))
            } else {
                format!("{m} r{a}, [r{b} + {d}]")
            }
        }
        Form::Reg => format!("{m} r{a}"),
        Form::Imm => format!("{m} {imm}"),
    }
}

/// Disassemble an image into reassemblable source (no address prefixes). Words
/// that do not decode are emitted as `.word` directives.
pub fn disasm_source(image: &[u8], origin: u32) -> String {
    let _ = origin;
    let mut out = String::new();
    let mut off = 0usize;
    while off + 8 <= image.len() {
        let mut bytes = [0u8; 8];
        bytes.copy_from_slice(&image[off..off + 8]);
        match Instr::decode(bytes) {
            Some(instr) => {
                out.push_str(&disasm_instr(instr));
                out.push('\n');
                off += 8;
            }
            None => {
                let w = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
                out.push_str(&format!(".word {w}\n"));
                off += 4;
            }
        }
    }
    out
}

/// Disassemble a whole image. Each line is prefixed with its address. Undecodable
/// words are shown as `.word` directives so the stream is always representable.
pub fn disasm(image: &[u8], origin: u32) -> String {
    let mut out = String::new();
    let mut off = 0usize;
    while off + 8 <= image.len() {
        let mut bytes = [0u8; 8];
        bytes.copy_from_slice(&image[off..off + 8]);
        let addr = origin + off as u32;
        match Instr::decode(bytes) {
            Some(instr) => {
                out.push_str(&format!("{:04x}: {}\n", addr, disasm_instr(instr)));
                off += 8;
            }
            None => {
                let w = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
                out.push_str(&format!("{addr:04x}: .word {w}\n"));
                off += 4;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_three_register_op() {
        let i = Instr {
            op: Op::Add,
            a: 0,
            b: 1,
            c: 2,
            imm: 0,
        };
        assert_eq!(disasm_instr(i), "add r0, r1, r2");
    }

    #[test]
    fn formats_memory_with_negative_displacement() {
        let i = Instr {
            op: Op::Load,
            a: 3,
            b: 2,
            c: 0,
            imm: (-4i32) as u32,
        };
        assert_eq!(disasm_instr(i), "load r3, [r2 - 4]");
    }

    #[test]
    fn formats_bare_op() {
        let i = Instr {
            op: Op::Iret,
            a: 0,
            b: 0,
            c: 0,
            imm: 0,
        };
        assert_eq!(disasm_instr(i), "iret");
    }
}
