//! Gate 1a: instruction semantics against a hand-specified reference, plus a
//! seeded differential fuzz that checks arithmetic flags against an independent
//! widening reference.

use bedrock::cpu::Cpu;
use bedrock::isa::{Instr, Op};

/// Execute a single instruction on a fresh CPU after applying `pre`.
fn exec(op: Op, a: u8, b: u8, c: u8, imm: u32, pre: impl FnOnce(&mut Cpu)) -> Cpu {
    let mut cpu = Cpu::new();
    pre(&mut cpu);
    let instr = Instr {
        op,
        a,
        b,
        c,
        imm,
    };
    cpu.load_image(0, &instr.encode());
    cpu.pc = 0;
    cpu.step(None);
    cpu
}

#[test]
fn add_basic() {
    let cpu = exec(Op::Add, 0, 1, 2, 0, |c| {
        c.regs[1] = 5;
        c.regs[2] = 7;
    });
    assert_eq!(cpu.regs[0], 12);
    assert!(!cpu.flags.zf && !cpu.flags.cf && !cpu.flags.sf && !cpu.flags.of);
}

#[test]
fn add_unsigned_carry() {
    let cpu = exec(Op::Add, 0, 1, 2, 0, |c| {
        c.regs[1] = 0xFFFF_FFFF;
        c.regs[2] = 1;
    });
    assert_eq!(cpu.regs[0], 0);
    assert!(cpu.flags.cf, "carry out expected");
    assert!(cpu.flags.zf, "result is zero");
    assert!(!cpu.flags.of, "no signed overflow");
}

#[test]
fn add_signed_overflow() {
    let cpu = exec(Op::Add, 0, 1, 2, 0, |c| {
        c.regs[1] = 0x7FFF_FFFF;
        c.regs[2] = 1;
    });
    assert_eq!(cpu.regs[0], 0x8000_0000);
    assert!(cpu.flags.of, "signed overflow expected");
    assert!(cpu.flags.sf, "result is negative");
    assert!(!cpu.flags.cf, "no unsigned carry");
}

#[test]
fn sub_borrow_and_zero() {
    let borrow = exec(Op::Sub, 0, 1, 2, 0, |c| {
        c.regs[1] = 3;
        c.regs[2] = 10;
    });
    assert_eq!(borrow.regs[0], 3u32.wrapping_sub(10));
    assert!(borrow.flags.cf, "borrow expected");
    assert!(borrow.flags.sf);

    let zero = exec(Op::Sub, 0, 1, 2, 0, |c| {
        c.regs[1] = 5;
        c.regs[2] = 5;
    });
    assert_eq!(zero.regs[0], 0);
    assert!(zero.flags.zf);
    assert!(!zero.flags.cf);
}

#[test]
fn sub_signed_overflow() {
    let cpu = exec(Op::Sub, 0, 1, 2, 0, |c| {
        c.regs[1] = 0x8000_0000;
        c.regs[2] = 1;
    });
    assert_eq!(cpu.regs[0], 0x7FFF_FFFF);
    assert!(cpu.flags.of, "MIN - 1 overflows");
}

#[test]
fn mul_overflow_flags() {
    let small = exec(Op::Mul, 0, 1, 2, 0, |c| {
        c.regs[1] = 6;
        c.regs[2] = 7;
    });
    assert_eq!(small.regs[0], 42);
    assert!(!small.flags.cf && !small.flags.of);

    let wide = exec(Op::Mul, 0, 1, 2, 0, |c| {
        c.regs[1] = 0x1_0000;
        c.regs[2] = 0x1_0000;
    });
    assert_eq!(wide.regs[0], 0, "low 32 bits of 2^32");
    assert!(wide.flags.cf && wide.flags.of, "product did not fit");
    assert!(wide.flags.zf);
}

#[test]
fn logic_and_shift() {
    let and = exec(Op::And, 0, 1, 2, 0, |c| {
        c.regs[1] = 0xF0F0;
        c.regs[2] = 0xFF00;
    });
    assert_eq!(and.regs[0], 0xF000);
    assert!(!and.flags.cf && !and.flags.of, "logic clears C and O");

    let shl = exec(Op::Shli, 0, 1, 0, 4, |c| c.regs[1] = 1);
    assert_eq!(shl.regs[0], 16);

    let shr = exec(Op::Shri, 0, 1, 0, 1, |c| c.regs[1] = 0x8000_0000);
    assert_eq!(shr.regs[0], 0x4000_0000);

    let not = exec(Op::Not, 0, 1, 0, 0, |c| c.regs[1] = 0);
    assert_eq!(not.regs[0], 0xFFFF_FFFF);

    let neg = exec(Op::Neg, 0, 1, 0, 0, |c| c.regs[1] = 1);
    assert_eq!(neg.regs[0], 0xFFFF_FFFF);
}

#[test]
fn cmp_sets_flags_without_writing() {
    let cpu = exec(Op::Cmp, 1, 2, 0, 0, |c| {
        c.regs[1] = 4;
        c.regs[2] = 9;
    });
    assert_eq!(cpu.regs[1], 4, "cmp must not modify operands");
    assert_eq!(cpu.regs[2], 9);
    assert!(cpu.flags.cf, "4 < 9 unsigned sets borrow");
    assert!(cpu.flags.sf);
    assert!(!cpu.flags.zf);
}

#[test]
fn conditional_branch_taken_and_not() {
    // JZ to 0x40 when zero flag is set.
    let taken = exec(Op::Jz, 0, 0, 0, 0x40, |c| c.flags.zf = true);
    assert_eq!(taken.pc, 0x40);

    let not_taken = exec(Op::Jz, 0, 0, 0, 0x40, |c| c.flags.zf = false);
    assert_eq!(not_taken.pc, 8, "falls through to next instruction");
}

#[test]
fn signed_branches() {
    // -1 < 1 signed: SF != OF after CMP.
    let mut cpu = Cpu::new();
    let prog = [
        Instr { op: Op::Cmp, a: 1, b: 2, c: 0, imm: 0 },
        Instr { op: Op::Jlt, a: 0, b: 0, c: 0, imm: 0x100 },
    ];
    let mut image = Vec::new();
    for i in prog {
        image.extend_from_slice(&i.encode());
    }
    cpu.load_image(0, &image);
    cpu.pc = 0;
    cpu.regs[1] = 0xFFFF_FFFF; // -1
    cpu.regs[2] = 1;
    cpu.step(None);
    cpu.step(None);
    assert_eq!(cpu.pc, 0x100, "jlt taken for -1 < 1");
}

#[test]
fn load_store_roundtrip() {
    let mut cpu = Cpu::new();
    let prog = [
        Instr { op: Op::Store, a: 1, b: 2, c: 0, imm: 4 },
        Instr { op: Op::Load, a: 3, b: 2, c: 0, imm: 4 },
    ];
    let mut image = Vec::new();
    for i in prog {
        image.extend_from_slice(&i.encode());
    }
    cpu.load_image(0, &image);
    cpu.pc = 0;
    cpu.regs[1] = 0xDEAD_BEEF;
    cpu.regs[2] = 0x1000;
    cpu.step(None);
    cpu.step(None);
    assert_eq!(cpu.read_word(0x1004), 0xDEAD_BEEF);
    assert_eq!(cpu.regs[3], 0xDEAD_BEEF);
}

#[test]
fn stack_push_pop_and_call_ret() {
    let mut cpu = Cpu::new();
    // push r1; pop r2
    let prog = [
        Instr { op: Op::Push, a: 1, b: 0, c: 0, imm: 0 },
        Instr { op: Op::Pop, a: 2, b: 0, c: 0, imm: 0 },
    ];
    let mut image = Vec::new();
    for i in prog {
        image.extend_from_slice(&i.encode());
    }
    cpu.load_image(0, &image);
    cpu.pc = 0;
    cpu.regs[1] = 0x1234_5678;
    let sp0 = cpu.sp;
    cpu.step(None);
    assert_eq!(cpu.sp, sp0 - 4);
    cpu.step(None);
    assert_eq!(cpu.regs[2], 0x1234_5678);
    assert_eq!(cpu.sp, sp0, "pop restores sp");

    // call 0x80 pushes return address, ret pops it.
    let mut cpu = Cpu::new();
    cpu.load_image(0, &Instr { op: Op::Call, a: 0, b: 0, c: 0, imm: 0x80 }.encode());
    cpu.load_image(0x80, &Instr { op: Op::Ret, a: 0, b: 0, c: 0, imm: 0 }.encode());
    cpu.pc = 0;
    cpu.step(None);
    assert_eq!(cpu.pc, 0x80);
    cpu.step(None);
    assert_eq!(cpu.pc, 8, "ret returns after the call");
}

/// Seeded differential fuzz: compare ADD/SUB result and flags to an independent
/// widening reference. Count comes from BEDROCK_FUZZ_OPS (default 5000).
#[test]
fn fuzz_arithmetic_matches_reference() {
    let n: u64 = std::env::var("BEDROCK_FUZZ_OPS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(5000);

    let mut seed: u64 = 0x9E37_79B9_7F4A_7C15;
    let mut next = || {
        // splitmix64
        seed = seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = seed;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        (z ^ (z >> 31)) as u32
    };

    for _ in 0..n {
        let x = next();
        let y = next();

        // ADD
        let cpu = exec(Op::Add, 0, 1, 2, 0, |c| {
            c.regs[1] = x;
            c.regs[2] = y;
        });
        let sum = (x as u64) + (y as u64);
        let ref_res = sum as u32;
        let ref_cf = sum > 0xFFFF_FFFF;
        let s = (x as i32 as i64) + (y as i32 as i64);
        let ref_of = s < i32::MIN as i64 || s > i32::MAX as i64;
        assert_eq!(cpu.regs[0], ref_res, "add result x={x:#x} y={y:#x}");
        assert_eq!(cpu.flags.cf, ref_cf, "add carry x={x:#x} y={y:#x}");
        assert_eq!(cpu.flags.of, ref_of, "add overflow x={x:#x} y={y:#x}");
        assert_eq!(cpu.flags.zf, ref_res == 0);
        assert_eq!(cpu.flags.sf, ref_res & 0x8000_0000 != 0);

        // SUB
        let cpu = exec(Op::Sub, 0, 1, 2, 0, |c| {
            c.regs[1] = x;
            c.regs[2] = y;
        });
        let ref_res = x.wrapping_sub(y);
        let ref_cf = (x as u64) < (y as u64);
        let d = (x as i32 as i64) - (y as i32 as i64);
        let ref_of = d < i32::MIN as i64 || d > i32::MAX as i64;
        assert_eq!(cpu.regs[0], ref_res, "sub result x={x:#x} y={y:#x}");
        assert_eq!(cpu.flags.cf, ref_cf, "sub borrow x={x:#x} y={y:#x}");
        assert_eq!(cpu.flags.of, ref_of, "sub overflow x={x:#x} y={y:#x}");
    }
}

#[test]
fn divide_and_remainder_by_zero_faults() {
    let dz = bedrock::cpu::FaultCode::DivideByZero as u32;
    for op in [Op::Div, Op::Rem] {
        let cpu = exec(op, 0, 1, 2, 0, |c| {
            c.regs[1] = 10;
            c.regs[2] = 0;
        });
        assert_eq!(cpu.regs[7], dz, "{op:?} by zero must raise the divide fault");
    }
    for op in [Op::Divi, Op::Remi] {
        let cpu = exec(op, 0, 1, 0, 0, |c| c.regs[1] = 10);
        assert_eq!(cpu.regs[7], dz, "{op:?} by zero must raise the divide fault");
    }

    // A normal division writes the quotient and does not fault.
    let ok = exec(Op::Div, 0, 1, 2, 0, |c| {
        c.regs[1] = 20;
        c.regs[2] = 6;
    });
    assert_eq!(ok.regs[0], 3);
    let rem = exec(Op::Rem, 0, 1, 2, 0, |c| {
        c.regs[1] = 20;
        c.regs[2] = 6;
    });
    assert_eq!(rem.regs[0], 2);
}

/// An independent reference for one ALU operation: result plus every flag,
/// derived from first principles and never sharing code with the CPU.
struct Ref {
    res: u32,
    zf: bool,
    sf: bool,
    cf: bool,
    of: bool,
}

fn nz(res: u32) -> (bool, bool) {
    (res == 0, res & 0x8000_0000 != 0)
}

fn ref_add(x: u32, y: u32) -> Ref {
    let wide = x as u64 + y as u64;
    let res = wide as u32;
    let (zf, sf) = nz(res);
    let s = x as i32 as i64 + y as i32 as i64;
    Ref {
        res,
        zf,
        sf,
        cf: wide > u32::MAX as u64,
        of: s < i32::MIN as i64 || s > i32::MAX as i64,
    }
}

fn ref_sub(x: u32, y: u32) -> Ref {
    let res = x.wrapping_sub(y);
    let (zf, sf) = nz(res);
    let d = x as i32 as i64 - y as i32 as i64;
    Ref {
        res,
        zf,
        sf,
        cf: (x as u64) < (y as u64),
        of: d < i32::MIN as i64 || d > i32::MAX as i64,
    }
}

fn ref_mul(x: u32, y: u32) -> Ref {
    let wide = x as u64 * y as u64;
    let res = wide as u32;
    let (zf, sf) = nz(res);
    let overflow = wide > u32::MAX as u64;
    Ref {
        res,
        zf,
        sf,
        cf: overflow,
        of: overflow,
    }
}

/// Logic and shift results set Z and S and always clear C and O.
fn ref_bit(res: u32) -> Ref {
    let (zf, sf) = nz(res);
    Ref {
        res,
        zf,
        sf,
        cf: false,
        of: false,
    }
}

fn check(cpu: &Cpu, r: &Ref, what: &str) {
    assert_eq!(cpu.regs[0], r.res, "{what}: result");
    assert_eq!(cpu.flags.zf, r.zf, "{what}: zf");
    assert_eq!(cpu.flags.sf, r.sf, "{what}: sf");
    assert_eq!(cpu.flags.cf, r.cf, "{what}: cf");
    assert_eq!(cpu.flags.of, r.of, "{what}: of");
}

/// Full-ALU differential: every arithmetic, logic, and shift opcode, in both
/// register-register and register-immediate forms, checked value-and-flags
/// against the independent reference over adversarial operands.
#[test]
fn fuzz_full_alu_matches_reference() {
    let n: u64 = std::env::var("BEDROCK_FUZZ_OPS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(4000);

    let mut seed: u64 = 0x2545_F491_4F6C_DD1D;
    let mut next = || {
        seed = seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = seed;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        (z ^ (z >> 31)) as u32
    };

    // Edge operands mixed in with random ones to stress boundaries.
    let edges = [
        0u32,
        1,
        0xFFFF_FFFF,
        0x8000_0000,
        0x7FFF_FFFF,
        2,
        31,
        32,
        33,
        0x0001_0000,
    ];
    let pick = |v: u32| -> u32 {
        if v.is_multiple_of(4) {
            edges[(v as usize / 4) % edges.len()]
        } else {
            v
        }
    };

    for _ in 0..n {
        let x = pick(next());
        let y = pick(next());
        let sh = next() % 40; // includes shift counts past the word width
        let d = if y == 0 { 0x9E37_79B1 } else { y }; // a nonzero divisor

        // Register-register forms.
        check(&exec(Op::Add, 0, 1, 2, 0, |c| { c.regs[1] = x; c.regs[2] = y; }), &ref_add(x, y), "add");
        check(&exec(Op::Sub, 0, 1, 2, 0, |c| { c.regs[1] = x; c.regs[2] = y; }), &ref_sub(x, y), "sub");
        check(&exec(Op::Mul, 0, 1, 2, 0, |c| { c.regs[1] = x; c.regs[2] = y; }), &ref_mul(x, y), "mul");
        check(&exec(Op::And, 0, 1, 2, 0, |c| { c.regs[1] = x; c.regs[2] = y; }), &ref_bit(x & y), "and");
        check(&exec(Op::Or, 0, 1, 2, 0, |c| { c.regs[1] = x; c.regs[2] = y; }), &ref_bit(x | y), "or");
        check(&exec(Op::Xor, 0, 1, 2, 0, |c| { c.regs[1] = x; c.regs[2] = y; }), &ref_bit(x ^ y), "xor");
        check(&exec(Op::Shl, 0, 1, 2, 0, |c| { c.regs[1] = x; c.regs[2] = sh; }), &ref_bit(x.wrapping_shl(sh)), "shl");
        check(&exec(Op::Shr, 0, 1, 2, 0, |c| { c.regs[1] = x; c.regs[2] = sh; }), &ref_bit(x.wrapping_shr(sh)), "shr");
        check(&exec(Op::Div, 0, 1, 2, 0, |c| { c.regs[1] = x; c.regs[2] = d; }), &ref_bit(x / d), "div");
        check(&exec(Op::Rem, 0, 1, 2, 0, |c| { c.regs[1] = x; c.regs[2] = d; }), &ref_bit(x % d), "rem");

        // Register-immediate forms.
        check(&exec(Op::Addi, 0, 1, 0, y, |c| c.regs[1] = x), &ref_add(x, y), "addi");
        check(&exec(Op::Subi, 0, 1, 0, y, |c| c.regs[1] = x), &ref_sub(x, y), "subi");
        check(&exec(Op::Muli, 0, 1, 0, y, |c| c.regs[1] = x), &ref_mul(x, y), "muli");
        check(&exec(Op::Andi, 0, 1, 0, y, |c| c.regs[1] = x), &ref_bit(x & y), "andi");
        check(&exec(Op::Ori, 0, 1, 0, y, |c| c.regs[1] = x), &ref_bit(x | y), "ori");
        check(&exec(Op::Xori, 0, 1, 0, y, |c| c.regs[1] = x), &ref_bit(x ^ y), "xori");
        check(&exec(Op::Shli, 0, 1, 0, sh, |c| c.regs[1] = x), &ref_bit(x.wrapping_shl(sh)), "shli");
        check(&exec(Op::Shri, 0, 1, 0, sh, |c| c.regs[1] = x), &ref_bit(x.wrapping_shr(sh)), "shri");
        check(&exec(Op::Divi, 0, 1, 0, d, |c| c.regs[1] = x), &ref_bit(x / d), "divi");
        check(&exec(Op::Remi, 0, 1, 0, d, |c| c.regs[1] = x), &ref_bit(x % d), "remi");

        // Unary forms.
        check(&exec(Op::Not, 0, 1, 0, 0, |c| c.regs[1] = x), &ref_bit(!x), "not");
        check(&exec(Op::Neg, 0, 1, 0, 0, |c| c.regs[1] = x), &ref_sub(0, x), "neg");

        // CMP / CMPI: reference is a subtraction whose result is discarded, so
        // flags must match SUB while r0 stays untouched.
        let cmp = exec(Op::Cmp, 1, 2, 0, 0, |c| { c.regs[1] = x; c.regs[2] = y; });
        let r = ref_sub(x, y);
        assert_eq!(cmp.flags.zf, r.zf, "cmp zf");
        assert_eq!(cmp.flags.sf, r.sf, "cmp sf");
        assert_eq!(cmp.flags.cf, r.cf, "cmp cf");
        assert_eq!(cmp.flags.of, r.of, "cmp of");
        assert_eq!(cmp.regs[1], x, "cmp must not write operands");
    }
}
