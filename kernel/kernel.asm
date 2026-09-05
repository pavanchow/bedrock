; Bedrock kernel: a preemptive round-robin scheduler for two user tasks.
;
; Boot installs the interrupt vector table, seeds a saved context for each
; task, then returns into task 0 in user mode. The timer interrupt preempts
; the running task and switches to the next runnable one. Tasks reach the
; kernel through the trap instruction to print, yield, and exit.
;
; The whole context of a suspended task lives on that task's own stack as
; [flags][pc][r0..r7]. A context switch is therefore just: save the current
; stack pointer into its task control block, load the next task's stack
; pointer, and iret.

.equ SYS_PRINT   1
.equ SYS_YIELD   2
.equ SYS_EXIT    3
.equ CONSOLE     0xF000
.equ NTASKS      2
.equ UFLAGS      0x30        ; FLAG_USER (0x20) | FLAG_IE (0x10)

; reset vector: execution begins at address 0.
    jmp _start

; ----------------------------------------------------------------------
; Boot
; ----------------------------------------------------------------------
_start:
    movi r0, kstack_top
    spset r0

    ; install the interrupt vector table
    movi r1, ivt
    movi r0, timer_handler
    store r0, [r1]
    movi r0, syscall_handler
    store r0, [r1 + 4]
    movi r0, fault_handler
    store r0, [r1 + 8]
    lidt ivt

    ; seed task 0's stack frame and save its stack pointer
    movi r0, task0_stack_top
    spset r0
    movi r0, UFLAGS
    push r0
    movi r0, task0_entry
    push r0
    movi r0, 0
    push r0
    push r0
    push r0
    push r0
    push r0
    push r0
    push r0
    push r0
    spget r0
    movi r1, tcb_sp
    store r0, [r1]

    ; seed task 1's stack frame and save its stack pointer
    movi r0, task1_stack_top
    spset r0
    movi r0, UFLAGS
    push r0
    movi r0, task1_entry
    push r0
    movi r0, 0
    push r0
    push r0
    push r0
    push r0
    push r0
    push r0
    push r0
    push r0
    spget r0
    movi r1, tcb_sp
    store r0, [r1 + 4]

    ; current task = 0, then launch it by restoring its frame
    movi r0, 0
    movi r1, cur_task
    store r0, [r1]
    movi r1, tcb_sp
    load r2, [r1]
    spset r2
    pop r7
    pop r6
    pop r5
    pop r4
    pop r3
    pop r2
    pop r1
    pop r0
    iret

; ----------------------------------------------------------------------
; Timer interrupt: preempt the running task and schedule the next one.
; ----------------------------------------------------------------------
timer_handler:
    push r0
    push r1
    push r2
    push r3
    push r4
    push r5
    push r6
    push r7
    jmp ctx_save

; ----------------------------------------------------------------------
; Syscall / trap handler. r6 holds the trap number, r1 the argument.
; ----------------------------------------------------------------------
syscall_handler:
    cmpi r6, SYS_PRINT
    jz sys_print
    ; yield and exit both switch tasks, so save the full context first
    push r0
    push r1
    push r2
    push r3
    push r4
    push r5
    push r6
    push r7
    cmpi r6, SYS_EXIT
    jz mark_exit
    jmp ctx_save

sys_print:
    movi r2, CONSOLE
    storeb r1, [r2]
    iret

mark_exit:
    movi r0, cur_task
    load r0, [r0]
    movi r1, tcb_state
    shli r2, r0, 2
    add r1, r1, r2
    movi r2, 1
    store r2, [r1]
    ; fall through into the scheduler

; Save the current stack pointer, choose the next runnable task, restore it.
ctx_save:
    movi r0, cur_task
    load r0, [r0]
    movi r1, tcb_sp
    shli r2, r0, 2
    add r1, r1, r2
    spget r2
    store r2, [r1]

    ; scan for the next runnable task starting after the current one
    movi r3, NTASKS
    movi r4, 0
pick_loop:
    addi r0, r0, 1
    cmp r0, r3
    jlt no_wrap
    movi r0, 0
no_wrap:
    movi r1, tcb_state
    shli r2, r0, 2
    add r1, r1, r2
    load r1, [r1]
    cmpi r1, 0
    jz found
    addi r4, r4, 1
    cmp r4, r3
    jlt pick_loop
    halt                    ; no runnable task remains

found:
    movi r1, cur_task
    store r0, [r1]
    movi r1, tcb_sp
    shli r2, r0, 2
    add r1, r1, r2
    load r2, [r1]
    spset r2
    pop r7
    pop r6
    pop r5
    pop r4
    pop r3
    pop r2
    pop r1
    pop r0
    iret

; ----------------------------------------------------------------------
; Fault handler: print '!' and halt. The demo tasks never trigger it.
; ----------------------------------------------------------------------
fault_handler:
    movi r2, CONSOLE
    movi r1, '!'
    storeb r1, [r2]
    halt

; ----------------------------------------------------------------------
; User task 0: print 'A' six times, doing busy work so the timer preempts it.
; ----------------------------------------------------------------------
task0_entry:
    movi r5, 0
t0_loop:
    movi r1, 'A'
    trap SYS_PRINT
    addi r5, r5, 1
    movi r2, 0
t0_busy:
    addi r2, r2, 1
    cmpi r2, 20
    jlt t0_busy
    cmpi r5, 6
    jlt t0_loop
    trap SYS_EXIT
t0_hang:
    jmp t0_hang

; ----------------------------------------------------------------------
; User task 1: print 'B' six times.
; ----------------------------------------------------------------------
task1_entry:
    movi r5, 0
t1_loop:
    movi r1, 'B'
    trap SYS_PRINT
    addi r5, r5, 1
    movi r2, 0
t1_busy:
    addi r2, r2, 1
    cmpi r2, 20
    jlt t1_busy
    cmpi r5, 6
    jlt t1_loop
    trap SYS_EXIT
t1_hang:
    jmp t1_hang

; ----------------------------------------------------------------------
; Data: vector table, task control blocks, and stacks.
; ----------------------------------------------------------------------
ivt:
    .space 12               ; three vectors: timer, syscall, fault
cur_task:
    .word 0
tcb_sp:
    .word 0, 0
tcb_state:
    .word 0, 0

    .space 256
kstack_top:
    .space 4

    .space 256
task0_stack_top:
    .space 4

    .space 256
task1_stack_top:
    .space 4
