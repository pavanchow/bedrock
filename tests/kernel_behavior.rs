//! Gate 3: the assembled kernel time-slices two tasks via the timer interrupt,
//! dispatches syscalls, and runs deterministically.

use bedrock::kernel::{assemble_kernel, run_kernel, RunConfig};

#[test]
fn kernel_assembles() {
    let asm = assemble_kernel();
    assert!(!asm.code.is_empty());
    for label in ["_start", "timer_handler", "syscall_handler", "cur_task", "tcb_sp"] {
        assert!(asm.labels.contains_key(label), "missing label {label}");
    }
}

#[test]
fn both_tasks_make_progress() {
    let report = run_kernel(&RunConfig::default());
    // Each task prints its own letter; both must appear multiple times, proving
    // each ran across more than one time slice.
    assert!(report.count('A') >= 2, "task 0 made no progress: {:?}", report.output);
    assert!(report.count('B') >= 2, "task 1 made no progress: {:?}", report.output);
    assert_eq!(report.count('A'), 6, "task 0 completed its work");
    assert_eq!(report.count('B'), 6, "task 1 completed its work");
    assert!(!report.output.contains('!'), "no fault occurred: {:?}", report.output);
}

#[test]
fn timer_drives_context_switches() {
    let report = run_kernel(&RunConfig::default());
    assert!(report.timer_interrupts >= 2, "timer must fire repeatedly");
    assert!(
        report.context_switches >= 2,
        "the running task must change more than once"
    );
}

#[test]
fn syscalls_are_dispatched() {
    let report = run_kernel(&RunConfig::default());
    // 12 prints plus 2 exits.
    assert_eq!(report.traps, 14, "every print and exit trapped into the kernel");
}

#[test]
fn tasks_actually_interleave() {
    // A purely sequential run (task 0 fully, then task 1) would be "AAAAAABBBBBB".
    // Preemption must interleave the letters.
    let report = run_kernel(&RunConfig::default());
    assert_ne!(
        report.output, "AAAAAABBBBBB",
        "tasks did not interleave under preemption"
    );
    assert!(report.output.contains("AB") || report.output.contains("BA"));
}

#[test]
fn kernel_halts_when_all_tasks_exit() {
    let report = run_kernel(&RunConfig::default());
    assert!(report.halted, "kernel should idle-halt after both tasks exit");
}

#[test]
fn run_is_deterministic() {
    let a = run_kernel(&RunConfig::default());
    let b = run_kernel(&RunConfig::default());
    assert_eq!(a.output, b.output, "same config, same output");
    assert_eq!(a.fingerprint, b.fingerprint, "same config, same final state");
    assert_eq!(a.cycles, b.cycles);
    assert_eq!(a.context_switches, b.context_switches);
}

#[test]
fn trace_is_deterministic() {
    let cfg = RunConfig {
        trace: true,
        ..RunConfig::default()
    };
    let a = run_kernel(&cfg);
    let b = run_kernel(&cfg);
    assert_eq!(a.trace, b.trace, "identical seed/config yields identical trace");
    assert!(a.trace.iter().any(|e| e.event == "TIMER"));
    assert!(a.trace.iter().any(|e| e.event.starts_with("TRAP")));
}
