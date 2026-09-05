//! Gate 2: interrupt and trap correctness.
//! - The timer transfers control through the vector table and iret restores
//!   the interrupted state exactly.
//! - A privileged instruction faults in user mode but not in kernel mode.
//! - A trap enters kernel mode and returns to the caller.

use bedrock::assembler::assemble;
use bedrock::cpu::{Cpu, FaultCode, StepEvent};

fn load(src: &str) -> Cpu {
    let asm = assemble(src).unwrap();
    let mut cpu = Cpu::new();
    cpu.load_image(asm.origin, &asm.code);
    cpu.pc = asm.origin;
    cpu
}

#[test]
fn timer_iret_restores_state_exactly() {
    // User code counts in r0. The timer handler at 0x200 does nothing but iret.
    let src = "\
        .org 0\n\
            addi r0, r0, 1\n\
            addi r0, r0, 1\n\
            addi r0, r0, 1\n\
            addi r0, r0, 1\n\
            addi r0, r0, 1\n\
        .org 0x100\n\
            .word 0x200, 0, 0\n\
        .org 0x200\n\
            iret\n";
    let mut cpu = load(src);
    cpu.ivt_base = 0x100;
    cpu.sp = 0x3000;
    cpu.flags.user = true;
    cpu.flags.ie = true;
    cpu.pc = 0;
    cpu.set_timer(3);

    // Run three user instructions, then snapshot right before the timer fires.
    while cpu.cycles < 3 {
        assert_eq!(cpu.step(None), StepEvent::Ran(bedrock::isa::Op::Addi));
    }
    let regs = cpu.regs;
    let pc = cpu.pc;
    let sp = cpu.sp;
    let flags = cpu.flags;

    // Next step delivers the timer through the vector table.
    let ev = cpu.step(None);
    assert_eq!(ev, StepEvent::TimerInterrupt);
    assert_eq!(cpu.pc, 0x200, "entered timer handler via vector table");
    assert!(!cpu.flags.user, "interrupt entered kernel mode");
    assert!(!cpu.flags.ie, "interrupts masked in handler");

    // The handler's iret restores everything.
    let ev = cpu.step(None);
    assert_eq!(ev, StepEvent::Ran(bedrock::isa::Op::Iret));
    assert_eq!(cpu.pc, pc, "pc restored exactly");
    assert_eq!(cpu.regs, regs, "registers restored exactly");
    assert_eq!(cpu.sp, sp, "stack pointer balanced");
    assert_eq!(cpu.flags, flags, "flags (incl. mode and IE) restored exactly");
}

#[test]
fn privileged_instruction_faults_in_user_mode() {
    // cli is privileged. In user mode it must fault through vector 2.
    let src = "\
        .org 0\n\
            cli\n\
        .org 0x100\n\
            .word 0, 0, 0x300\n\
        .org 0x300\n\
            halt\n";
    let mut cpu = load(src);
    cpu.ivt_base = 0x100;
    cpu.sp = 0x3000;
    cpu.flags.user = true;
    cpu.pc = 0;

    let ev = cpu.step(None);
    assert_eq!(ev, StepEvent::Fault(FaultCode::Privileged));
    assert_eq!(cpu.pc, 0x300, "entered fault handler via vector table");
    assert!(!cpu.flags.user, "fault handler runs in kernel mode");
    assert_eq!(cpu.regs[7], FaultCode::Privileged as u32, "fault code delivered");
}

#[test]
fn privileged_instruction_ok_in_kernel_mode() {
    let mut cpu = load("cli\nhalt\n");
    cpu.flags.user = false;
    cpu.flags.ie = true;
    cpu.pc = 0;
    let ev = cpu.step(None);
    assert_eq!(ev, StepEvent::Ran(bedrock::isa::Op::Cli));
    assert!(!cpu.flags.ie, "cli cleared interrupt enable");
}

#[test]
fn trap_enters_kernel_and_returns() {
    // User traps with number 7. Handler at 0x400 records nothing and returns.
    let src = "\
        .org 0\n\
            trap 7\n\
            addi r1, r1, 99\n\
        .org 0x100\n\
            .word 0, 0x400, 0\n\
        .org 0x400\n\
            iret\n";
    let mut cpu = load(src);
    cpu.ivt_base = 0x100;
    cpu.sp = 0x3000;
    cpu.flags.user = true;
    cpu.flags.ie = true;
    cpu.pc = 0;

    let ev = cpu.step(None);
    assert_eq!(ev, StepEvent::Trap(7));
    assert_eq!(cpu.regs[6], 7, "trap number delivered in r6");
    assert_eq!(cpu.pc, 0x400, "entered syscall handler via vector table");
    assert!(!cpu.flags.user, "trap entered kernel mode");

    // Handler returns to the instruction after the trap, back in user mode.
    let ev = cpu.step(None);
    assert_eq!(ev, StepEvent::Ran(bedrock::isa::Op::Iret));
    assert_eq!(cpu.pc, 8, "returned after the trap");
    assert!(cpu.flags.user, "back in user mode");

    // And the following user instruction runs normally.
    cpu.step(None);
    assert_eq!(cpu.regs[1], 99);
}

#[test]
fn timer_does_not_fire_when_interrupts_disabled() {
    let mut cpu = load("addi r0, r0, 1\naddi r0, r0, 1\naddi r0, r0, 1\n");
    cpu.ivt_base = 0x100;
    cpu.flags.ie = false; // masked
    cpu.pc = 0;
    cpu.set_timer(1);
    for _ in 0..3 {
        let ev = cpu.step(None);
        assert!(matches!(ev, StepEvent::Ran(_)), "no interrupt while masked");
    }
    assert_eq!(cpu.regs[0], 3);
}
