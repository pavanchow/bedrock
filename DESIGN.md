# Bedrock design

This document describes the Bedrock machine, its instruction set, the interrupt
and trap model, the user and kernel mode split, the assembler, the scheduler
written in Bedrock assembly, and why each correctness gate proves what it claims.

## Why an emulated machine

A kernel written in real machine assembly boots on hardware. It cannot be loaded
into a Rust test harness and it cannot run in a browser. To make a kernel in
assembly that is fully testable and fully visualizable, Bedrock defines its own
machine and its own assembly language, then runs the kernel on that machine in
software. Nothing is faked. The CPU decodes real bytes, the assembler produces
those bytes from text, and the kernel is an assembly source file. Owning the
machine is what lets a test assert that a context switch restored every register
exactly, or that the same seed produces the same trace to the cycle.

## The machine

State:

- Eight 32-bit general purpose registers, `r0` through `r7`.
- A program counter `pc` and a stack pointer `sp`.
- A flags register with six bits: zero, carry, sign, overflow, interrupt enable,
  and a user/kernel mode bit.
- A flat 64 KiB little-endian byte memory.
- An interrupt vector table base register set by `lidt`.

The stack grows downward. `push` decrements `sp` by four and writes a word.
`pop` reads a word and increments `sp` by four.

A memory-mapped console lives at address `0xF000`. A byte written there with
`storeb` is appended to the machine's output stream. This is the only device,
and it is how the kernel prints without any special print opcode.

## Instruction encoding

Every instruction is exactly eight bytes.

```
byte 0      byte 1   byte 2   byte 3   bytes 4..8
[ opcode ] [  a   ] [  b   ] [  c   ] [ imm32 little-endian ]
```

Only the low three bits of each register field are used. The fixed width keeps
the fetch, the assembler, and the disassembler simple, and it makes the
round-trip property easy to state and to test. The opcode byte is the enum
discriminant, so a decoded opcode always re-encodes to the same byte.

## The instruction set

Arithmetic and logic come in a register-register form such as `add rd, ra, rb`
and a register-immediate form such as `addi rd, ra, imm`. The set covers add,
subtract, multiply, and, or, xor, shift left, shift right, not, and negate.

Flag rules:

- Add sets carry on unsigned overflow and sets overflow on signed overflow.
- Subtract and compare set carry on borrow, meaning the left operand was smaller
  when read as unsigned, and set overflow on signed overflow.
- Multiply sets carry and overflow together when the product does not fit in 32
  bits.
- The logical and shift operations clear carry and overflow.
- All of them set zero from the result and sign from bit 31.

`cmp` and `cmpi` compute a subtraction only to set the flags and never write a
register. Branches read the flags. Alongside the raw flag branches such as `jz`
and `jc`, the set includes signed comparison branches `jlt`, `jge`, `jle`, and
`jgt`, which read the sign and overflow flags together the way a real CPU does
after a compare.

`load` and `store` move a word between a register and `[rb + imm]`. `loadb` and
`storeb` move a single byte. `call` pushes the return address and jumps, and
`ret` pops it. `spget` and `spset` read and write the stack pointer, which the
kernel needs to switch stacks during a context switch.

## User and kernel mode

The mode bit lives in the flags register. Five instructions are privileged:
`lidt`, `sti`, `cli`, `iret`, and `halt`. If a privileged instruction is fetched
while the mode bit says user, the CPU does not execute it. Instead it raises a
fault. This is the protection boundary. User tasks can compute, branch, use the
stack, and trap into the kernel, but they cannot disable interrupts, load the
vector table, return from an interrupt, or halt the machine.

`trap` is deliberately not privileged. It is the doorway a user task uses to ask
the kernel for service.

## Interrupts and traps

There are three vectors. Index 0 is the timer, index 1 is the syscall or trap,
and index 2 is the fault. Each entry is a 32-bit address stored at
`ivt_base + index * 4`.

Delivery is the same for all three. The CPU pushes the current flags, then the
current program counter, then switches to kernel mode, clears the interrupt
enable bit, and loads `pc` from the vector. Pushing flags before pc means the
saved frame reads, from the top of the stack downward, as pc then flags.

`iret` reverses exactly that. It pops the program counter, then pops the flags.
Because the mode bit and the interrupt enable bit are part of the flags word,
`iret` restores the caller's mode and re-enables interrupts in one step. Clearing
the interrupt enable bit on entry is what prevents the timer from firing again
while a handler runs, so handlers do not re-enter.

The timer fires every N retired instructions, where N is the timer period, but
only when the interrupt enable bit is set. During a handler that bit is clear, so
the timer is naturally masked until the handler returns.

`trap n` places the trap number in `r6`, then delivers through the syscall
vector. The kernel handler reads `r6` to decide which service was requested.

## The assembler

The assembler runs in two passes. The first pass walks the source, assigns an
address to every instruction and data directive, and records the address of each
label and the value of each `.equ` constant. Instruction sizes are fixed and
data directive sizes are known from their contents, so the first pass never needs
a label value. The second pass emits bytes with every label resolved.

It understands `.org` to set the location, `.word` and `.byte` to place data,
`.string` to place text, `.space` to reserve zeroed bytes, and `.equ` to define a
constant. Memory operands are `[rb]`, `[rb + imm]`, and `[rb - imm]`. Immediates
can be decimal, hex with `0x`, binary with `0b`, a character literal, or a label
or constant with an optional offset such as `base + 8`.

The disassembler is the inverse and shares the operand-form table with the
assembler, so the two cannot drift apart.

## The kernel and its scheduler

The kernel is `kernel/kernel.asm`, assembled at build time and embedded in the
binary. Its whole job is to run two user tasks as if they were running at the
same time, by rapidly switching between them.

The key idea is that the entire context of a suspended task lives on that task's
own stack. When the timer interrupts a task, the CPU has already pushed that
task's flags and pc onto the task's stack. The handler then pushes `r0` through
`r7` on top. At that point the task's complete state is a run of ten words on its
stack, and the only thing the kernel must remember elsewhere is where the top of
that run is. So a task control block is just one saved stack pointer per task,
plus a runnable-or-exited flag.

A context switch is therefore three moves. Save the current stack pointer into
the current task's control block with `spget`. Choose the next runnable task.
Load that task's saved stack pointer with `spset`. Then pop `r0` through `r7` and
`iret`. The pops and the `iret` read from the new task's stack, so control
returns into the new task exactly where it was suspended, in user mode, with its
registers and flags intact.

Boot builds a fake suspended frame for each task by hand. It sets the stack
pointer to the task's stack, pushes an initial flags word with the user and
interrupt-enable bits set, pushes the task entry address as the saved pc, and
pushes eight zero registers. It saves that stack pointer into the task control
block. After seeding both tasks it launches task 0 by loading its stack pointer
and running the same pop-and-`iret` path a real switch uses. From that moment the
timer drives everything.

The trap handler dispatches on the trap number in `r6`. Print writes the
character in `r1` to the console port and returns. Yield runs the same scheduler
path as the timer. Exit marks the current task as exited and schedules another,
and when no task remains runnable the scheduler halts the machine.

The two demo tasks each print their own letter six times with a short busy loop
between prints, so the timer preempts them repeatedly and their output
interleaves. A trace of a run shows the pattern clearly:

```
   62  01f0   K     0    TIMER     timer preempts task 0, enters kernel mode
   63  01f8   K     0    push      handler saves the task's registers
  ...
  108  0238   K     1    TRAP 1    task 1, now running, asks the kernel to print
```

## Why each gate proves its claim

Gate one, instruction semantics. Each representative case fixes inputs and
asserts the resulting registers and flags against values worked out by hand,
including the unsigned carry edge, the signed overflow edge, and taken and
not-taken branches. The seeded fuzz recomputes add and subtract results, carry,
and overflow with a widening 64-bit reference that shares no code with the CPU,
so agreement across thousands of random inputs is real evidence, not a tautology.
The assembler byte tests pin the encoding, and the round-trip test assembles the
disassembly of an image and checks the bytes are unchanged, which can only hold
if the encoder, the decoder, and both text directions all agree.

Gate two, interrupts and traps. The timer test snapshots every register, the
flags, the stack pointer, and the program counter at the instant before the timer
fires, lets the interrupt and an empty handler run, and asserts the state after
`iret` equals the snapshot exactly. That is the precise meaning of a correct
save and restore. A separate test runs a privileged instruction in user mode and
asserts a fault through the fault vector, then runs the same instruction in kernel
mode and asserts it executes, which proves the protection boundary is real and
not merely present. A trap test asserts the mode flips to kernel on entry and
back to user on return, and that execution resumes after the trap.

Gate three, kernel behavior. Running the real assembled kernel, the tests assert
that both letters appear multiple times, which means both tasks ran across more
than one time slice. They assert the output is not the fully sequential ordering,
which means preemption actually interleaved the tasks. They count the timer
interrupts and the changes of the running task to confirm the switches are driven
by the timer, and they count the traps to confirm every print and exit went
through the trap handler. Finally they run the kernel twice and assert identical
output, identical cycle count, and an identical state fingerprint, which is the
determinism claim stated as an equality the machine either meets or fails.
