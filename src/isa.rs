//! The Bedrock instruction set architecture.
//!
//! A Bedrock machine has eight 32-bit general purpose registers (R0..R7), a
//! program counter, a stack pointer, and a single flags register that also
//! carries the interrupt-enable and user/kernel mode bits. Memory is a flat
//! little-endian byte array. Every instruction is a fixed eight bytes:
//!
//! ```text
//! byte 0      byte 1   byte 2   byte 3   bytes 4..8
//! [ opcode ] [  a   ] [  b   ] [  c   ] [ imm32 (LE) ]
//! ```
//!
//! Only the low three bits of the register fields are meaningful.

/// Number of general purpose registers.
pub const NUM_REGS: usize = 8;

/// Fixed instruction width in bytes.
pub const INSTR_SIZE: u32 = 8;

/// Default linear memory size (64 KiB).
pub const MEM_SIZE: usize = 0x1_0000;

/// Interrupt vector table entry indices.
pub const VEC_TIMER: u32 = 0;
pub const VEC_SYSCALL: u32 = 1;
pub const VEC_FAULT: u32 = 2;

/// Flags register bit positions.
pub const FLAG_ZF: u32 = 1 << 0;
pub const FLAG_CF: u32 = 1 << 1;
pub const FLAG_SF: u32 = 1 << 2;
pub const FLAG_OF: u32 = 1 << 3;
pub const FLAG_IE: u32 = 1 << 4;
pub const FLAG_USER: u32 = 1 << 5;

/// The decoded flags register.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Flags {
    pub zf: bool,
    pub cf: bool,
    pub sf: bool,
    pub of: bool,
    /// Interrupt enable.
    pub ie: bool,
    /// True in user mode, false in kernel mode.
    pub user: bool,
}

impl Flags {
    pub fn to_u32(self) -> u32 {
        let mut v = 0;
        if self.zf {
            v |= FLAG_ZF;
        }
        if self.cf {
            v |= FLAG_CF;
        }
        if self.sf {
            v |= FLAG_SF;
        }
        if self.of {
            v |= FLAG_OF;
        }
        if self.ie {
            v |= FLAG_IE;
        }
        if self.user {
            v |= FLAG_USER;
        }
        v
    }

    pub fn from_u32(v: u32) -> Self {
        Flags {
            zf: v & FLAG_ZF != 0,
            cf: v & FLAG_CF != 0,
            sf: v & FLAG_SF != 0,
            of: v & FLAG_OF != 0,
            ie: v & FLAG_IE != 0,
            user: v & FLAG_USER != 0,
        }
    }
}

/// Every Bedrock opcode. The discriminant is the byte written to memory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Op {
    Nop = 0x00,
    Mov = 0x01,
    Movi = 0x02,
    Add = 0x03,
    Sub = 0x04,
    Mul = 0x05,
    And = 0x06,
    Or = 0x07,
    Xor = 0x08,
    Shl = 0x09,
    Shr = 0x0A,
    Addi = 0x0B,
    Subi = 0x0C,
    Muli = 0x0D,
    Andi = 0x0E,
    Ori = 0x0F,
    Xori = 0x10,
    Shli = 0x11,
    Shri = 0x12,
    Not = 0x13,
    Neg = 0x14,
    Cmp = 0x15,
    Cmpi = 0x16,
    Load = 0x17,
    Store = 0x18,
    Loadb = 0x19,
    Storeb = 0x1A,
    Jmp = 0x1B,
    Jz = 0x1C,
    Jnz = 0x1D,
    Jc = 0x1E,
    Jnc = 0x1F,
    Js = 0x20,
    Jns = 0x21,
    Jo = 0x22,
    Jno = 0x23,
    Jlt = 0x24,
    Jge = 0x25,
    Jle = 0x26,
    Jgt = 0x27,
    Push = 0x28,
    Pop = 0x29,
    Call = 0x2A,
    Ret = 0x2B,
    Lidt = 0x2C,
    Sti = 0x2D,
    Cli = 0x2E,
    Iret = 0x2F,
    Trap = 0x30,
    Halt = 0x31,
    Spget = 0x32,
    Spset = 0x33,
    Div = 0x34,
    Divi = 0x35,
    Rem = 0x36,
    Remi = 0x37,
}

/// How an opcode's textual operands map onto the encoded fields. Used by both
/// the assembler and the disassembler so the two can never disagree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Form {
    /// No operands: NOP, RET, STI, CLI, IRET, HALT.
    None,
    /// rd, ra, rb (three registers): the reg-reg ALU ops.
    Rrr,
    /// rd, ra, imm: the reg-imm ALU ops.
    Rri,
    /// rd, ra: MOV, NOT, NEG.
    Rr,
    /// rd, imm: MOVI.
    Ri,
    /// ra, rb: CMP.
    Rr2,
    /// ra, imm: CMPI.
    Ri2,
    /// rd, [ra + imm]: LOAD, LOADB.
    MemLoad,
    /// rs, [ra + imm]: STORE, STOREB.
    MemStore,
    /// single register: PUSH, POP.
    Reg,
    /// single immediate (address or vector): jumps, CALL, JMP, LIDT, TRAP.
    Imm,
}

impl Op {
    /// Decode a byte into an opcode.
    pub fn from_u8(b: u8) -> Option<Op> {
        use Op::*;
        let op = match b {
            0x00 => Nop,
            0x01 => Mov,
            0x02 => Movi,
            0x03 => Add,
            0x04 => Sub,
            0x05 => Mul,
            0x06 => And,
            0x07 => Or,
            0x08 => Xor,
            0x09 => Shl,
            0x0A => Shr,
            0x0B => Addi,
            0x0C => Subi,
            0x0D => Muli,
            0x0E => Andi,
            0x0F => Ori,
            0x10 => Xori,
            0x11 => Shli,
            0x12 => Shri,
            0x13 => Not,
            0x14 => Neg,
            0x15 => Cmp,
            0x16 => Cmpi,
            0x17 => Load,
            0x18 => Store,
            0x19 => Loadb,
            0x1A => Storeb,
            0x1B => Jmp,
            0x1C => Jz,
            0x1D => Jnz,
            0x1E => Jc,
            0x1F => Jnc,
            0x20 => Js,
            0x21 => Jns,
            0x22 => Jo,
            0x23 => Jno,
            0x24 => Jlt,
            0x25 => Jge,
            0x26 => Jle,
            0x27 => Jgt,
            0x28 => Push,
            0x29 => Pop,
            0x2A => Call,
            0x2B => Ret,
            0x2C => Lidt,
            0x2D => Sti,
            0x2E => Cli,
            0x2F => Iret,
            0x30 => Trap,
            0x31 => Halt,
            0x32 => Spget,
            0x33 => Spset,
            0x34 => Div,
            0x35 => Divi,
            0x36 => Rem,
            0x37 => Remi,
            _ => return None,
        };
        Some(op)
    }

    /// The canonical lowercase mnemonic.
    pub fn mnemonic(self) -> &'static str {
        use Op::*;
        match self {
            Nop => "nop",
            Mov => "mov",
            Movi => "movi",
            Add => "add",
            Sub => "sub",
            Mul => "mul",
            And => "and",
            Or => "or",
            Xor => "xor",
            Shl => "shl",
            Shr => "shr",
            Addi => "addi",
            Subi => "subi",
            Muli => "muli",
            Andi => "andi",
            Ori => "ori",
            Xori => "xori",
            Shli => "shli",
            Shri => "shri",
            Not => "not",
            Neg => "neg",
            Cmp => "cmp",
            Cmpi => "cmpi",
            Load => "load",
            Store => "store",
            Loadb => "loadb",
            Storeb => "storeb",
            Jmp => "jmp",
            Jz => "jz",
            Jnz => "jnz",
            Jc => "jc",
            Jnc => "jnc",
            Js => "js",
            Jns => "jns",
            Jo => "jo",
            Jno => "jno",
            Jlt => "jlt",
            Jge => "jge",
            Jle => "jle",
            Jgt => "jgt",
            Push => "push",
            Pop => "pop",
            Call => "call",
            Ret => "ret",
            Lidt => "lidt",
            Sti => "sti",
            Cli => "cli",
            Iret => "iret",
            Trap => "trap",
            Halt => "halt",
            Spget => "spget",
            Spset => "spset",
            Div => "div",
            Divi => "divi",
            Rem => "rem",
            Remi => "remi",
        }
    }

    /// Look an opcode up by mnemonic.
    pub fn from_mnemonic(s: &str) -> Option<Op> {
        use Op::*;
        // Aliases: jeq/jne read naturally for equality comparisons.
        let op = match s {
            "nop" => Nop,
            "mov" => Mov,
            "movi" => Movi,
            "add" => Add,
            "sub" => Sub,
            "mul" => Mul,
            "and" => And,
            "or" => Or,
            "xor" => Xor,
            "shl" => Shl,
            "shr" => Shr,
            "addi" => Addi,
            "subi" => Subi,
            "muli" => Muli,
            "andi" => Andi,
            "ori" => Ori,
            "xori" => Xori,
            "shli" => Shli,
            "shri" => Shri,
            "not" => Not,
            "neg" => Neg,
            "cmp" => Cmp,
            "cmpi" => Cmpi,
            "load" => Load,
            "store" => Store,
            "loadb" => Loadb,
            "storeb" => Storeb,
            "jmp" => Jmp,
            "jz" | "jeq" => Jz,
            "jnz" | "jne" => Jnz,
            "jc" => Jc,
            "jnc" => Jnc,
            "js" => Js,
            "jns" => Jns,
            "jo" => Jo,
            "jno" => Jno,
            "jlt" => Jlt,
            "jge" => Jge,
            "jle" => Jle,
            "jgt" => Jgt,
            "push" => Push,
            "pop" => Pop,
            "call" => Call,
            "ret" => Ret,
            "lidt" => Lidt,
            "sti" => Sti,
            "cli" => Cli,
            "iret" => Iret,
            "trap" => Trap,
            "halt" => Halt,
            "spget" => Spget,
            "spset" => Spset,
            "div" => Div,
            "divi" => Divi,
            "rem" => Rem,
            "remi" => Remi,
            _ => return None,
        };
        Some(op)
    }

    /// The operand form for this opcode.
    pub fn form(self) -> Form {
        use Op::*;
        match self {
            Nop | Ret | Sti | Cli | Iret | Halt => Form::None,
            Add | Sub | Mul | And | Or | Xor | Shl | Shr | Div | Rem => Form::Rrr,
            Addi | Subi | Muli | Andi | Ori | Xori | Shli | Shri | Divi | Remi => Form::Rri,
            Mov | Not | Neg => Form::Rr,
            Movi => Form::Ri,
            Cmp => Form::Rr2,
            Cmpi => Form::Ri2,
            Load | Loadb => Form::MemLoad,
            Store | Storeb => Form::MemStore,
            Push | Pop | Spget | Spset => Form::Reg,
            Jmp | Jz | Jnz | Jc | Jnc | Js | Jns | Jo | Jno | Jlt | Jge | Jle | Jgt | Call
            | Lidt | Trap => Form::Imm,
        }
    }

    /// True for instructions that only kernel mode may execute.
    pub fn is_privileged(self) -> bool {
        use Op::*;
        matches!(self, Lidt | Sti | Cli | Iret | Halt)
    }
}

/// A fully decoded instruction word.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Instr {
    pub op: Op,
    pub a: u8,
    pub b: u8,
    pub c: u8,
    pub imm: u32,
}

impl Instr {
    /// Encode into eight little-endian bytes.
    pub fn encode(self) -> [u8; 8] {
        let imm = self.imm.to_le_bytes();
        [
            self.op as u8,
            self.a,
            self.b,
            self.c,
            imm[0],
            imm[1],
            imm[2],
            imm[3],
        ]
    }

    /// Decode eight bytes into an instruction, if the opcode is valid.
    pub fn decode(bytes: [u8; 8]) -> Option<Instr> {
        let op = Op::from_u8(bytes[0])?;
        let imm = u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]);
        Some(Instr {
            op,
            a: bytes[1],
            b: bytes[2],
            c: bytes[3],
            imm,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_decode_roundtrip() {
        let i = Instr {
            op: Op::Addi,
            a: 3,
            b: 5,
            c: 0,
            imm: 0xDEAD_BEEF,
        };
        assert_eq!(Instr::decode(i.encode()), Some(i));
    }

    #[test]
    fn opcode_byte_roundtrip() {
        for b in 0u8..=0xFF {
            if let Some(op) = Op::from_u8(b) {
                assert_eq!(op as u8, b, "opcode {b:#x} is not its own discriminant");
            }
        }
    }

    #[test]
    fn mnemonic_roundtrip() {
        for b in 0u8..=0x37 {
            if let Some(op) = Op::from_u8(b) {
                assert_eq!(Op::from_mnemonic(op.mnemonic()), Some(op));
            }
        }
    }

    #[test]
    fn flags_bit_roundtrip() {
        let f = Flags {
            zf: true,
            cf: false,
            sf: true,
            of: false,
            ie: true,
            user: true,
        };
        assert_eq!(Flags::from_u32(f.to_u32()), f);
        assert_eq!(f.to_u32(), FLAG_ZF | FLAG_SF | FLAG_IE | FLAG_USER);
    }
}
