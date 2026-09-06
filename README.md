# Bedrock

A from-scratch virtual CPU, an assembler for it, and a small operating system
kernel written in that assembly. The kernel boots on the emulated machine,
installs an interrupt vector table, and time-slices between two user tasks using
a timer interrupt and a round-robin scheduler, servicing syscalls through a trap
handler.

Live playground: https://pavanchow.github.io/bedrock/

## The honest framing

A real bootable kernel written in machine assembly cannot be exercised by
`cargo test` in CI, and it cannot run inside a web page. Booting real hardware or
even QEMU is not something a unit test or a browser can assert against.

Bedrock takes the honest route and owns the whole stack. It is:

1. A virtual CPU, a machine with its own registers, flags, memory, and
   instruction set, emulated in Rust.
2. An assembler and disassembler for that instruction set.
3. A kernel written in Bedrock assembly, assembled at build time and executed on
   the emulated CPU.

So Bedrock genuinely is a kernel in assembly. It is assembly for our own
instruction set, running on our own emulator. Because we own the machine, every
claim about scheduling, interrupts, privilege levels, and determinism can be
asserted directly against observed state. That is the gap it fills. It is a place
to learn how a kernel, interrupts, context switching, and a scheduler actually
work, where you can single-step the whole thing and watch it happen.

## Quickstart

```
cargo build --release

# Run the bundled kernel demo: two tasks time-sliced by the timer interrupt.
cargo run --release -- kernel

# Watch it step by step (mode, task, and event per instruction).
cargo run --release -- kernel --trace

# Assemble and run your own program, with a trace and a timer.
cargo run --release -- run path/to/program.asm --trace --timer 50

# Assemble to hex, or disassemble.
cargo run --release -- asm path/to/program.asm
cargo run --release -- disasm path/to/program.asm
```

Running `bedrock kernel` prints the console output produced by the two tasks
(the letters `A` and `B` interleaved as they are preempted), plus a summary of
timer interrupts, context switches, syscalls, and a fingerprint of the final
state that is identical on every run.

## The machine

- Eight 32-bit general purpose registers `r0`..`r7`, a program counter, and a
  stack pointer.
- A flags register holding zero, carry, sign, and overflow, plus the interrupt
  enable bit and the user/kernel mode bit.
- A flat 64 KiB little-endian memory.
- A memory-mapped console at address `0xF000`. A byte stored there is emitted to
  the output stream, which is how the kernel prints.

## The instruction set

Every instruction is a fixed eight bytes: one opcode byte, three register-field
bytes, and a 32-bit little-endian immediate.

| Group | Instructions |
| --- | --- |
| Move | `mov`, `movi` |
| Arithmetic and logic (reg-reg) | `add`, `sub`, `mul`, `div`, `rem`, `and`, `or`, `xor`, `shl`, `shr`, `not`, `neg` |
| Arithmetic and logic (reg-imm) | `addi`, `subi`, `muli`, `divi`, `remi`, `andi`, `ori`, `xori`, `shli`, `shri` |
| Compare | `cmp`, `cmpi` |
| Memory | `load`, `store`, `loadb`, `storeb` |
| Branch | `jmp`, `jz`/`jeq`, `jnz`/`jne`, `jc`, `jnc`, `js`, `jns`, `jo`, `jno`, `jlt`, `jge`, `jle`, `jgt` |
| Stack and calls | `push`, `pop`, `call`, `ret`, `spget`, `spset` |
| Privileged | `lidt`, `sti`, `cli`, `iret`, `halt` |
| Trap | `trap` |

The assembler understands labels, the directives `.org`, `.word`, `.byte`,
`.string`, `.space`, and `.equ`, memory operands of the form `[rb]`,
`[rb + imm]`, and `[rb - imm]`, and immediates written in decimal, hex, binary,
as a character literal, or as a label with an offset.

`div` and `rem` are unsigned. A divide or remainder by zero raises a fault
through the fault vector rather than doing anything undefined.

Privileged instructions attempted in user mode raise a fault through the fault
vector. A trap switches to kernel mode and jumps through the syscall vector,
which is how a user task asks the kernel to print, yield, or exit.

## Memory safety of the guest

The emulator is a sandbox. Nothing a guest program does can panic the host or
touch memory outside the emulated 64 KiB. A load or store outside memory, a bad
opcode, a privileged instruction in user mode, and a divide by zero all raise a
clean guest fault through the fault vector. A stack pointer or vector base driven
out of range faults too, and if the fault itself cannot be delivered because the
kernel stack or the vector table is unusable, the machine raises a double fault
and halts instead of recursing or reading out of bounds. The assembler is
bounded the same way. It rejects any program whose addresses overflow or whose
image would grow past the 64 KiB memory, so a malformed source file returns a
clean error instead of allocating without limit.

## The correctness gate

The tests are the proof that each claim holds. They are bounded for CI and
tunable with `BEDROCK_FUZZ_OPS` and `BEDROCK_CYCLES`.

1. Instruction semantics. Each instruction's effect on registers, memory, and
   flags is checked against a hand-specified reference, including carry,
   overflow, and branch edge cases. A seeded differential fuzz compares every
   arithmetic, logic, and shift opcode, in both register-register and
   register-immediate forms, value and every flag, against an independent
   reference over adversarial operands. The assembler emits the expected bytes
   for known snippets, and `assemble(disassemble(assemble(x)))` reproduces the
   exact bytes.
2. Interrupts and traps. The timer transfers control through the vector table
   and `iret` restores the interrupted registers, flags, stack pointer, and
   program counter exactly. A privileged instruction faults in user mode but not
   in kernel mode. A trap enters kernel mode and returns to the caller.
3. Kernel behavior. Running the assembled kernel, both tasks make progress
   across multiple time slices, context switches happen via the timer, syscalls
   are dispatched through the trap handler, and the run is deterministic: the
   same configuration yields an identical trace and final state.
4. Adversarial stress. Random and malformed instruction streams, wild stack
   pointers, vector bases, and program counters, out-of-bounds memory access,
   privileged ops in user mode, near-infinite loops, and malformed assembly text
   are fuzzed against the emulator and the assembler. The gate is that none of it
   panics the host, hangs it, or reads or writes out of bounds. Every illegal
   act resolves to a clean guest fault, a double fault that halts, or a clean
   assembler error. A bounded cycle budget guarantees every run terminates.

Run it all with `cargo test`. The fuzzing gates scale with `BEDROCK_FUZZ_OPS`
and the kernel cycle budget with `BEDROCK_CYCLES`. See [DESIGN.md](DESIGN.md) for
the full ISA, the interrupt and trap model, the scheduler written in assembly,
and why each gate proves its claim.
