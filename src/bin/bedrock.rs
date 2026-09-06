//! The Bedrock command line tool: assemble, run, disassemble, and the kernel demo.
#![warn(clippy::pedantic)]
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_possible_wrap,
    clippy::cast_lossless,
    clippy::too_many_lines,
    clippy::wildcard_imports,
    clippy::enum_glob_use
)]

use std::fmt::Write;
use std::process::ExitCode;

use bedrock::assembler::assemble;
use bedrock::cpu::{Cpu, StepEvent};
use bedrock::disasm::{disasm, disasm_instr};
use bedrock::isa::{Flags, Instr, INSTR_SIZE};
use bedrock::kernel::{run_kernel, RunConfig};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        usage();
        return ExitCode::FAILURE;
    }

    let result = match args[1].as_str() {
        "run" => cmd_run(&args[2..]),
        "kernel" => cmd_kernel(&args[2..]),
        "asm" => cmd_asm(&args[2..]),
        "disasm" => cmd_disasm(&args[2..]),
        "help" | "-h" | "--help" => {
            usage();
            Ok(())
        }
        other => Err(format!("unknown command '{other}'")),
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn usage() {
    println!(
        "bedrock - a virtual CPU, assembler, and kernel\n\
         \n\
         USAGE:\n\
         \x20 bedrock run <file.asm> [--timer N] [--trace] [--cycles N]\n\
         \x20 bedrock kernel [--timer N] [--trace] [--cycles N]\n\
         \x20 bedrock asm <file.asm>          assemble and print hex bytes\n\
         \x20 bedrock disasm <file.asm>       assemble then disassemble\n\
         \x20 bedrock help"
    );
}

fn flag_value(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

fn has_flag(args: &[String], name: &str) -> bool {
    args.iter().any(|a| a == name)
}

fn first_positional(args: &[String]) -> Option<&String> {
    args.iter().find(|a| !a.starts_with("--"))
}

fn cmd_run(args: &[String]) -> Result<(), String> {
    let path = first_positional(args).ok_or("run needs a file path")?;
    let src = std::fs::read_to_string(path).map_err(|e| format!("cannot read {path}: {e}"))?;
    let asm = assemble(&src)?;

    let timer: u64 = flag_value(args, "--timer").map_or(0, |v| v.parse().unwrap_or(0));
    let cycles: u64 =
        flag_value(args, "--cycles").map_or(100_000, |v| v.parse().unwrap_or(100_000));
    let trace = has_flag(args, "--trace");

    let mut cpu = Cpu::new();
    cpu.load_image(asm.origin, &asm.code);
    cpu.pc = asm.origin;
    if timer > 0 {
        cpu.set_timer(timer);
    }

    while cpu.cycles < cycles && !cpu.halted {
        let pc = cpu.pc;
        let event = cpu.step(None);
        if trace {
            let instr = decode_at(&cpu, pc);
            let text = instr.map_or_else(|| "?".to_string(), disasm_instr);
            println!(
                "{:5}  {:04x}  {:<22}  {}",
                cpu.cycles,
                pc,
                text,
                describe(&event)
            );
        }
        if matches!(event, StepEvent::Halted) {
            break;
        }
    }

    print_state(&cpu);
    Ok(())
}

fn decode_at(cpu: &Cpu, pc: u32) -> Option<Instr> {
    let a = pc as usize;
    if a + 8 > cpu.mem.len() {
        return None;
    }
    let mut b = [0u8; 8];
    b.copy_from_slice(&cpu.mem[a..a + 8]);
    Instr::decode(b)
}

fn describe(event: &StepEvent) -> String {
    match event {
        StepEvent::Ran(op) => op.mnemonic().to_string(),
        StepEvent::TimerInterrupt => "-> TIMER".to_string(),
        StepEvent::Trap(n) => format!("-> TRAP {n}"),
        StepEvent::Fault(c) => format!("-> FAULT {c:?}"),
        StepEvent::DoubleFault => "-> DOUBLEFAULT".to_string(),
        StepEvent::Halted => "HALT".to_string(),
    }
}

fn print_state(cpu: &Cpu) {
    println!("--- final state ---");
    for row in 0..2 {
        let mut line = String::new();
        for col in 0..4 {
            let i = row * 4 + col;
            let _ = write!(line, "r{i}={:#010x}  ", cpu.regs[i]);
        }
        println!("{}", line.trim_end());
    }
    println!("pc={:#06x}  sp={:#06x}", cpu.pc, cpu.sp);
    println!("flags={}  cycles={}", flags_str(cpu.flags), cpu.cycles);
    if !cpu.output.is_empty() {
        println!("output: {}", String::from_utf8_lossy(&cpu.output));
    }
}

fn flags_str(f: Flags) -> String {
    let bit = |b: bool, c: char| if b { c } else { '-' };
    format!(
        "[{}{}{}{}{}{}]",
        bit(f.zf, 'Z'),
        bit(f.cf, 'C'),
        bit(f.sf, 'S'),
        bit(f.of, 'O'),
        bit(f.ie, 'I'),
        bit(f.user, 'U'),
    )
}

fn cmd_kernel(args: &[String]) -> Result<(), String> {
    let mut cfg = RunConfig::default().with_env();
    if let Some(v) = flag_value(args, "--timer") {
        cfg.timer_period = v.parse().map_err(|_| "bad --timer")?;
    }
    if let Some(v) = flag_value(args, "--cycles") {
        cfg.max_cycles = v.parse().map_err(|_| "bad --cycles")?;
    }
    cfg.trace = has_flag(args, "--trace");

    let report = run_kernel(&cfg);

    if cfg.trace {
        println!("cycle   pc    mode  task  event");
        for e in &report.trace {
            println!(
                "{:5}  {:04x}   {}     {}    {}",
                e.cycle,
                e.pc,
                if e.user_mode { "U" } else { "K" },
                e.cur_task,
                e.event
            );
        }
        println!();
    }

    println!("=== Bedrock kernel demo ===");
    println!("console output   : {:?}", report.output);
    println!("task 0 ('A') runs: {}", report.count('A'));
    println!("task 1 ('B') runs: {}", report.count('B'));
    println!("timer interrupts : {}", report.timer_interrupts);
    println!("context switches : {}", report.context_switches);
    println!("syscalls (traps) : {}", report.traps);
    println!("cycles executed  : {}", report.cycles);
    println!("halted (idle)    : {}", report.halted);
    println!("state fingerprint: {:#018x}", report.fingerprint);
    Ok(())
}

fn cmd_asm(args: &[String]) -> Result<(), String> {
    let path = first_positional(args).ok_or("asm needs a file path")?;
    let src = std::fs::read_to_string(path).map_err(|e| format!("cannot read {path}: {e}"))?;
    let asm = assemble(&src)?;
    println!("origin: {:#06x}  size: {} bytes", asm.origin, asm.code.len());
    for (i, chunk) in asm.code.chunks(8).enumerate() {
        let addr = asm.origin as usize + i * 8;
        let hex: Vec<String> = chunk.iter().map(|b| format!("{b:02x}")).collect();
        println!("{:04x}: {}", addr, hex.join(" "));
    }
    Ok(())
}

fn cmd_disasm(args: &[String]) -> Result<(), String> {
    let path = first_positional(args).ok_or("disasm needs a file path")?;
    let src = std::fs::read_to_string(path).map_err(|e| format!("cannot read {path}: {e}"))?;
    let asm = assemble(&src)?;
    print!("{}", disasm(&asm.code, asm.origin));
    let _ = INSTR_SIZE;
    Ok(())
}
