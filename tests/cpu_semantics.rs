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
