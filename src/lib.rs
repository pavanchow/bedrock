//! # Bedrock
//!
//! Bedrock is a from-scratch virtual machine with its own instruction set, an
//! assembler and disassembler for it, and a small operating-system kernel
//! written in that assembly. The kernel boots on the emulated CPU, installs an
//! interrupt vector table, and time-slices between two user tasks using a timer
//! interrupt and a round-robin scheduler, servicing syscalls through a trap
//! handler.
//!
//! A real bootable kernel in machine assembly cannot be exercised inside a unit
//! test or a browser. Bedrock sidesteps that by owning the whole stack: the CPU
//! is emulated, the assembly is our own ISA, so every claim about scheduling,
//! interrupts, and privilege can be asserted directly against observed state.

pub mod assembler;
pub mod cpu;
pub mod disasm;
pub mod isa;
pub mod kernel;

pub use assembler::{assemble, Assembled};
pub use cpu::{Cpu, FaultCode, StepEvent};
pub use disasm::{disasm, disasm_instr, disasm_source};
pub use isa::{Flags, Instr, Op, MEM_SIZE, NUM_REGS};
pub use kernel::{assemble_kernel, run_kernel, KernelReport, RunConfig, KERNEL_SRC};

/// Run an already-loaded machine until it halts or `max_cycles` retire.
/// Returns the number of cycles executed.
pub fn run_until_halt(cpu: &mut Cpu, max_cycles: u64) -> u64 {
    while cpu.cycles < max_cycles && !cpu.halted {
        cpu.step(None);
    }
    cpu.cycles
}
