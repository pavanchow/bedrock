//! The bundled Bedrock kernel: assembling it, running it, and reporting on the
//! round-robin scheduler's behaviour.

use crate::assembler::{assemble, Assembled};
use crate::cpu::{Cpu, StepEvent};

/// The kernel source, assembled at build/run time from real Bedrock assembly.
pub const KERNEL_SRC: &str = include_str!("../kernel/kernel.asm");

/// How to run the kernel.
#[derive(Debug, Clone)]
pub struct RunConfig {
    /// Hard cap on retired instructions so a run always terminates.
    pub max_cycles: u64,
    /// Timer interrupt period in retired instructions.
    pub timer_period: u64,
    /// Record a step-by-step trace.
    pub trace: bool,
}

impl Default for RunConfig {
    fn default() -> Self {
        RunConfig {
            max_cycles: 20_000,
            timer_period: 80,
            trace: false,
        }
    }
}

impl RunConfig {
    /// Apply the BEDROCK_CYCLES environment override, if set.
    pub fn with_env(mut self) -> Self {
        if let Ok(v) = std::env::var("BEDROCK_CYCLES") {
            if let Ok(n) = v.parse::<u64>() {
                self.max_cycles = n;
            }
        }
        self
    }
}

/// A single recorded step, used for the trace and the determinism gate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TraceEntry {
    pub cycle: u64,
    pub pc: u32,
    pub user_mode: bool,
    pub cur_task: u32,
    pub event: String,
}

/// The outcome of running the kernel.
#[derive(Debug, Clone)]
pub struct KernelReport {
    /// Console output produced by the print syscall.
    pub output: String,
    pub cycles: u64,
    /// Number of timer interrupts delivered (each is a scheduling point).
    pub timer_interrupts: u64,
    /// Number of traps (syscalls) dispatched.
    pub traps: u64,
    /// Times the running task index actually changed.
    pub context_switches: u64,
    /// Whether the machine halted (both tasks exited) within the cycle budget.
    pub halted: bool,
    pub trace: Vec<TraceEntry>,
    /// Final architectural fingerprint for the determinism gate.
    pub fingerprint: u64,
}

impl KernelReport {
    /// Count of a given output byte, e.g. per-task progress.
    pub fn count(&self, ch: char) -> usize {
        self.output.chars().filter(|&c| c == ch).count()
    }
}

/// Assemble the bundled kernel image.
pub fn assemble_kernel() -> Assembled {
    assemble(KERNEL_SRC).expect("bundled kernel must assemble")
}

/// The address of `cur_task` in the assembled kernel.
fn cur_task_addr(asm: &Assembled) -> u32 {
    *asm
        .labels
        .get("cur_task")
        .expect("kernel exposes cur_task")
}

/// Assemble and run the kernel to completion (or the cycle cap).
pub fn run_kernel(cfg: &RunConfig) -> KernelReport {
    let asm = assemble_kernel();
    let cur_addr = cur_task_addr(&asm);

    let mut cpu = Cpu::new();
    cpu.load_image(asm.origin, &asm.code);
    cpu.pc = asm.origin;
    cpu.set_timer(cfg.timer_period);

    let mut report = KernelReport {
        output: String::new(),
        cycles: 0,
        timer_interrupts: 0,
        traps: 0,
        context_switches: 0,
        halted: false,
        trace: Vec::new(),
        fingerprint: 0,
    };

    let mut last_task = cpu.read_word(cur_addr);
    while cpu.cycles < cfg.max_cycles {
        let event = cpu.step(None);
        let cur_task = cpu.read_word(cur_addr);
        match &event {
            StepEvent::TimerInterrupt => report.timer_interrupts += 1,
            StepEvent::Trap(_) => report.traps += 1,
            StepEvent::Halted => {
                report.halted = true;
            }
            _ => {}
        }
        if cur_task != last_task {
            report.context_switches += 1;
            last_task = cur_task;
        }
        if cfg.trace {
            report.trace.push(TraceEntry {
                cycle: cpu.cycles,
                pc: cpu.pc,
                user_mode: cpu.flags.user,
                cur_task,
                event: describe(&event),
            });
        }
        if matches!(event, StepEvent::Halted) {
            break;
        }
    }

    report.output = String::from_utf8_lossy(&cpu.output).into_owned();
    report.cycles = cpu.cycles;
    report.fingerprint = fingerprint(&cpu);
    report
}

fn describe(event: &StepEvent) -> String {
    match event {
        StepEvent::Ran(op) => op.mnemonic().to_string(),
        StepEvent::TimerInterrupt => "TIMER".to_string(),
        StepEvent::Trap(n) => format!("TRAP {n}"),
        StepEvent::Fault(c) => format!("FAULT {c:?}"),
        StepEvent::Halted => "HALT".to_string(),
    }
}

/// A stable fingerprint of the architectural state (FNV-1a over key registers
/// and the console output).
fn fingerprint(cpu: &Cpu) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    let mut mix = |v: u32| {
        for b in v.to_le_bytes() {
            h ^= b as u64;
            h = h.wrapping_mul(0x100000001b3);
        }
    };
    for r in cpu.regs {
        mix(r);
    }
    mix(cpu.pc);
    mix(cpu.sp);
    mix(cpu.flags.to_u32());
    for &b in &cpu.output {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}
