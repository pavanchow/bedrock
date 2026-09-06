//! Adversarial stress gate: random and malformed instruction streams, wild
//! register/stack/vector state, out-of-bounds memory access, privileged ops in
//! user mode, and malformed assembly text. Nothing a guest program or a bad
//! source file can do may panic the host, hang it, or read/write outside the
//! emulated memory. Every illegal act inside the guest must resolve to a clean
//! guest fault, a double fault that halts, or a clean assembler error.
//!
//! Scale with BEDROCK_FUZZ_OPS (default 4000). Footprint is bounded: each seed
//! runs a fixed step budget and the assembler is capped at 64 KiB.

use bedrock::assembler::assemble;
use bedrock::cpu::{Cpu, StepEvent};
use bedrock::isa::MEM_SIZE;

fn fuzz_ops() -> u64 {
    std::env::var("BEDROCK_FUZZ_OPS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(4000)
}

/// splitmix64, seeded per test so runs are deterministic and reproducible.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    fn u32(&mut self) -> u32 {
        self.next() as u32
    }
    fn below(&mut self, n: u32) -> u32 {
        self.u32() % n
    }
}

/// Run a machine for a fixed step budget and assert the run terminates cleanly.
/// The step budget is the hang guard: any guest that loops forever is bounded.
fn run_bounded(cpu: &mut Cpu, budget: u64) {
    for _ in 0..budget {
        let ev = cpu.step(None);
        if matches!(ev, StepEvent::Halted | StepEvent::DoubleFault) {
            return;
        }
    }
}

/// Random instruction streams over fully randomized architectural state. Wild
/// SP, IVT base, PC, and user/kernel mode exercise every stack, vector, and
/// memory path. A host panic, overflow, or OOB access fails the test.
#[test]
fn fuzz_random_instruction_streams() {
    let n = fuzz_ops();
    let mut rng = Rng(0xF00D_BEEF_1234_5678);

    for _ in 0..n {
        let mut cpu = Cpu::new();
        // Fill memory with random bytes: a mix of valid and invalid opcodes.
        for b in cpu.mem.iter_mut() {
            *b = rng.u32() as u8;
        }
        for r in cpu.regs.iter_mut() {
            *r = rng.u32();
        }
        // Wild but in-type stack pointer, vector base, and program counter.
        cpu.sp = rng.u32();
        cpu.ivt_base = rng.u32();
        cpu.pc = rng.u32();
        cpu.flags.user = rng.u32() & 1 == 0;
        cpu.flags.ie = rng.u32() & 1 == 0;
        if rng.u32() & 3 == 0 {
            cpu.set_timer((rng.below(8) + 1) as u64);
        }
        run_bounded(&mut cpu, 300);
        // sp and pc are u32 by construction; the invariant we assert is simply
        // that we returned without panicking and cycles stayed bounded.
        assert!(cpu.cycles <= 300, "step budget must bound execution");
    }
}

/// Targeted adversarial programs, each a documented host-panic candidate.
#[test]
fn adversarial_programs_fault_cleanly() {
    use bedrock::isa::{Instr, Op};
    let enc = |op, a, b, c, imm| Instr { op, a, b, c, imm }.encode();

    // 1. Wild stack pointer then PUSH: the push cannot touch host memory. The
    //    fault handler needs the same (broken) stack, so this correctly becomes
    //    a double fault that halts, never a host panic or OOB write.
    {
        let mut cpu = Cpu::new();
        cpu.regs[0] = 0xFFFF_FFFE;
        let mut img = enc(Op::Spset, 0, 0, 0, 0).to_vec();
        img.extend_from_slice(&enc(Op::Push, 0, 0, 0, 0));
        cpu.load_image(0, &img);
        cpu.pc = 0;
        assert_eq!(cpu.step(None), StepEvent::Ran(Op::Spset));
        assert_eq!(cpu.step(None), StepEvent::DoubleFault);
        assert!(cpu.halted);
    }

    // 2. IVT base past memory then TRAP: the vector entry cannot be read, so
    //    the trap escalates to a double fault that halts instead of panicking.
    {
        let mut cpu = Cpu::new();
        let mut img = enc(Op::Lidt, 0, 0, 0, MEM_SIZE as u32).to_vec();
        img.extend_from_slice(&enc(Op::Trap, 0, 0, 0, 1));
        cpu.load_image(0, &img);
        cpu.pc = 0;
        cpu.sp = 0x8000;
        assert_eq!(cpu.step(None), StepEvent::Ran(Op::Lidt));
        assert_eq!(cpu.step(None), StepEvent::DoubleFault);
        assert!(cpu.halted);
    }

    // 3. Out-of-bounds LOAD (well above the 64 KiB memory): clean memory fault.
    {
        let mut cpu = Cpu::new();
        cpu.regs[1] = 0xFFFF_0000;
        cpu.load_image(0, &enc(Op::Load, 0, 1, 0, 0));
        cpu.pc = 0;
        cpu.ivt_base = 0; // valid vector table region
        assert_eq!(cpu.step(None), StepEvent::Fault(bedrock::cpu::FaultCode::Memory));
    }

    // 4. RET with a wild stack pointer: no OOB read. The broken stack also
    //    prevents fault delivery, so the machine double faults and halts.
    {
        let mut cpu = Cpu::new();
        cpu.sp = 0xFFFF_FFFF;
        cpu.load_image(0, &enc(Op::Ret, 0, 0, 0, 0));
        cpu.pc = 0;
        assert_eq!(cpu.step(None), StepEvent::DoubleFault);
        assert!(cpu.halted);
    }

    // 5. Privileged instruction in user mode: clean privilege fault.
    {
        let mut cpu = Cpu::new();
        cpu.flags.user = true;
        cpu.sp = 0x8000;
        cpu.load_image(0, &enc(Op::Cli, 0, 0, 0, 0));
        cpu.pc = 0;
        assert_eq!(
            cpu.step(None),
            StepEvent::Fault(bedrock::cpu::FaultCode::Privileged)
        );
    }

    // 6. Unbounded self-jump (near-infinite loop): the step budget bounds it.
    {
        let mut cpu = Cpu::new();
        cpu.load_image(0, &enc(Op::Jmp, 0, 0, 0, 0));
        cpu.pc = 0;
        run_bounded(&mut cpu, 500);
        assert_eq!(cpu.cycles, 500, "infinite loop bounded by the step budget");
    }
}

/// Malformed assembly text must always return Ok or Err, never panic or hang.
#[test]
fn fuzz_malformed_assembly() {
    let n = fuzz_ops();
    let mut rng = Rng(0xABCD_1234_5678_9ABC);

    let atoms = [
        "mov", "add", "movi", "load", "store", "jmp", "trap", "r0", "r7", "r9", "r99", "[r1",
        "r2]", "[r3 + 4]", "[r0 -", "0xDEADBEEF", "0x", "'", "'A'", ",", ":", ";", "\"", "\"str",
        ".org", ".word", ".byte", ".space", ".equ", ".string", "+", "-", "1000", "-5", "label",
        "0b1010", "frob", "", "  ", "\n",
    ];

    for _ in 0..n {
        let lines = rng.below(6) + 1;
        let mut src = String::new();
        for _ in 0..lines {
            let toks = rng.below(5) + 1;
            for _ in 0..toks {
                src.push_str(atoms[rng.below(atoms.len() as u32) as usize]);
                src.push(' ');
            }
            src.push('\n');
        }
        // The contract: a Result, never a panic. Values are irrelevant here.
        let _ = assemble(&src);
    }
}

/// The assembler must reject images that would overflow addresses or exceed the
/// emulated memory, rather than overflowing arithmetic or allocating gigabytes.
#[test]
fn assembler_rejects_oversized_and_overflowing_images() {
    assert!(
        assemble(".space 0xFFFFFFFF").is_err(),
        ".space of 4 GiB must be rejected, not allocated"
    );
    assert!(
        assemble(".space 0x20000").is_err(),
        "image larger than 64 KiB must be rejected"
    );
    assert!(
        assemble(".org 0xFFFFFFFF\nnop").is_err(),
        ".org at the top of the address space then an instruction overflows"
    );
    assert!(
        assemble(".org 0x20000\nnop").is_err(),
        ".org past memory must be rejected"
    );
    assert!(
        assemble("movi r0, 0xFFFFFFFFFF").is_err(),
        "immediate that does not fit in 32 bits must be rejected"
    );
    assert!(
        assemble("mov r9, r0").is_err(),
        "register index out of range must be rejected"
    );
    assert!(
        assemble(".string \"unterminated").is_err(),
        "unterminated string literal must be rejected"
    );

    // A `.org` that jumps backward is legal and must not underflow pass-2 math.
    let asm = assemble(".org 100\n nop\n .org 0\n halt\n").unwrap();
    assert_eq!(asm.origin, 0, "origin is the lowest address touched");
    assert!(asm.code.len() as usize <= MEM_SIZE);
}
