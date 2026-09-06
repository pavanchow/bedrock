//! The Bedrock virtual CPU: fetch, decode, execute, plus interrupts and traps.

use crate::isa::*;

/// Reasons a fault is raised through the fault vector.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FaultCode {
    /// A privileged instruction was attempted in user mode.
    Privileged = 1,
    /// A memory access fell outside linear memory.
    Memory = 2,
    /// An undecodable opcode was fetched.
    BadOpcode = 3,
    /// A DIV or REM by zero was attempted.
    DivideByZero = 4,
}

/// What happened during a single `step`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StepEvent {
    /// A normal instruction retired.
    Ran(Op),
    /// The timer fired and control entered the timer handler.
    TimerInterrupt,
    /// A trap (syscall) entered the syscall handler.
    Trap(u32),
    /// A fault entered the fault handler.
    Fault(FaultCode),
    /// A fault could not be delivered because the vector table or the kernel
    /// stack is itself unusable. The machine halts rather than recursing or
    /// touching host memory out of bounds.
    DoubleFault,
    /// The machine executed HALT.
    Halted,
}

/// A callback the host installs to service syscalls that touch the outside
/// world. Returning `false` means the syscall was not host-handled and the
/// trap should proceed through the vector table to kernel code.
pub type Syscall = dyn FnMut(&mut Cpu, u32) -> bool;

/// The virtual machine.
pub struct Cpu {
    pub regs: [u32; NUM_REGS],
    pub pc: u32,
    pub sp: u32,
    pub flags: Flags,
    pub ivt_base: u32,
    pub mem: Vec<u8>,
    pub cycles: u64,
    /// Timer fires every `timer_period` retired instructions. Zero disables it.
    pub timer_period: u64,
    pub halted: bool,
    /// Bytes written by the print syscall, in order.
    pub output: Vec<u8>,
    /// Memory-mapped console port: byte stores here append to `output`.
    pub console_out: u32,
    next_timer: u64,
}

impl Cpu {
    /// A fresh machine in kernel mode with interrupts disabled.
    pub fn new() -> Self {
        Cpu {
            regs: [0; NUM_REGS],
            pc: 0,
            sp: (MEM_SIZE as u32) - 4,
            flags: Flags::default(),
            ivt_base: 0,
            mem: vec![0; MEM_SIZE],
            cycles: 0,
            timer_period: 0,
            halted: false,
            output: Vec::new(),
            console_out: 0xF000,
            next_timer: 0,
        }
    }

    /// Load a machine-code image at `addr`.
    pub fn load_image(&mut self, addr: u32, image: &[u8]) {
        let start = addr as usize;
        let end = start + image.len();
        assert!(end <= self.mem.len(), "image does not fit in memory");
        self.mem[start..end].copy_from_slice(image);
    }

    /// Arm the timer so it fires every `period` retired instructions.
    pub fn set_timer(&mut self, period: u64) {
        self.timer_period = period;
        self.next_timer = self.cycles + period;
    }

    // ---- memory helpers -------------------------------------------------

    fn in_bounds(&self, addr: u32, len: u32) -> bool {
        (addr as u64) + (len as u64) <= self.mem.len() as u64
    }

    pub fn read_word(&self, addr: u32) -> u32 {
        let a = addr as usize;
        u32::from_le_bytes([self.mem[a], self.mem[a + 1], self.mem[a + 2], self.mem[a + 3]])
    }

    pub fn write_word(&mut self, addr: u32, val: u32) {
        let a = addr as usize;
        self.mem[a..a + 4].copy_from_slice(&val.to_le_bytes());
    }

    /// Push a word, faulting cleanly if the stack pointer leaves memory.
    fn try_push(&mut self, val: u32) -> Result<(), ()> {
        let sp = self.sp.wrapping_sub(4);
        if !self.in_bounds(sp, 4) {
            return Err(());
        }
        self.sp = sp;
        self.write_word(sp, val);
        Ok(())
    }

    /// Pop a word, faulting cleanly if the stack pointer is out of memory.
    fn try_pop(&mut self) -> Result<u32, ()> {
        if !self.in_bounds(self.sp, 4) {
            return Err(());
        }
        let v = self.read_word(self.sp);
        self.sp = self.sp.wrapping_add(4);
        Ok(v)
    }

    // ---- interrupt delivery --------------------------------------------

    /// Push the interrupt frame (flags then pc), enter kernel mode with
    /// interrupts masked, and jump through vector `index`. Returns `Err` if the
    /// vector table entry or the two stack slots fall outside memory, so the
    /// caller can escalate to a double fault instead of touching host memory.
    fn try_enter_vector(&mut self, index: u32) -> Result<(), ()> {
        let vec_addr = self.ivt_base.wrapping_add(index.wrapping_mul(4));
        if !self.in_bounds(vec_addr, 4) {
            return Err(());
        }
        let sp1 = self.sp.wrapping_sub(4);
        let sp2 = sp1.wrapping_sub(4);
        if !self.in_bounds(sp1, 4) || !self.in_bounds(sp2, 4) {
            return Err(());
        }
        let target = self.read_word(vec_addr);
        self.sp = sp2;
        self.write_word(sp1, self.flags.to_u32());
        self.write_word(sp2, self.pc);
        self.flags.user = false;
        self.flags.ie = false;
        self.pc = target;
        Ok(())
    }

    fn fault(&mut self, code: FaultCode) -> StepEvent {
        self.regs[7] = code as u32;
        if self.try_enter_vector(VEC_FAULT).is_err() {
            self.halted = true;
            return StepEvent::DoubleFault;
        }
        StepEvent::Fault(code)
    }

    // ---- the step function ---------------------------------------------

    /// Execute one instruction (or deliver one pending interrupt). The
    /// optional `syscall` hook is consulted on TRAP before the kernel vector.
    pub fn step(&mut self, syscall: Option<&mut Syscall>) -> StepEvent {
        use Op::*;

        if self.halted {
            return StepEvent::Halted;
        }

        // A due timer interrupt preempts the next instruction.
        if self.timer_period != 0 && self.flags.ie && self.cycles >= self.next_timer {
            self.next_timer = self.cycles.wrapping_add(self.timer_period);
            if self.try_enter_vector(VEC_TIMER).is_err() {
                self.halted = true;
                return StepEvent::DoubleFault;
            }
            return StepEvent::TimerInterrupt;
        }

        if !self.in_bounds(self.pc, INSTR_SIZE) {
            return self.fault(FaultCode::Memory);
        }

        let base = self.pc as usize;
        let mut bytes = [0u8; 8];
        bytes.copy_from_slice(&self.mem[base..base + 8]);
        let Some(instr) = Instr::decode(bytes) else {
            return self.fault(FaultCode::BadOpcode);
        };

        if instr.op.is_privileged() && self.flags.user {
            return self.fault(FaultCode::Privileged);
        }

        // Advance past the instruction before executing so relative flow is
        // measured from the following instruction, matching call/ret.
        self.pc = self.pc.wrapping_add(INSTR_SIZE);
        self.cycles += 1;

        let a = (instr.a & 0x07) as usize;
        let b = (instr.b & 0x07) as usize;
        let c = (instr.c & 0x07) as usize;

        match instr.op {
            Nop => {}
            Mov => self.regs[a] = self.regs[b],
            Movi => self.regs[a] = instr.imm,
            Add => self.regs[a] = self.alu_add(self.regs[b], self.regs[c]),
            Sub => self.regs[a] = self.alu_sub(self.regs[b], self.regs[c]),
            Mul => self.regs[a] = self.alu_mul(self.regs[b], self.regs[c]),
            And => self.regs[a] = self.alu_bit(self.regs[b] & self.regs[c]),
            Or => self.regs[a] = self.alu_bit(self.regs[b] | self.regs[c]),
            Xor => self.regs[a] = self.alu_bit(self.regs[b] ^ self.regs[c]),
            Shl => self.regs[a] = self.alu_bit(self.regs[b].wrapping_shl(self.regs[c])),
            Shr => self.regs[a] = self.alu_bit(self.regs[b].wrapping_shr(self.regs[c])),
            Addi => self.regs[a] = self.alu_add(self.regs[b], instr.imm),
            Subi => self.regs[a] = self.alu_sub(self.regs[b], instr.imm),
            Muli => self.regs[a] = self.alu_mul(self.regs[b], instr.imm),
            Andi => self.regs[a] = self.alu_bit(self.regs[b] & instr.imm),
            Ori => self.regs[a] = self.alu_bit(self.regs[b] | instr.imm),
            Xori => self.regs[a] = self.alu_bit(self.regs[b] ^ instr.imm),
            Shli => self.regs[a] = self.alu_bit(self.regs[b].wrapping_shl(instr.imm)),
            Shri => self.regs[a] = self.alu_bit(self.regs[b].wrapping_shr(instr.imm)),
            Not => self.regs[a] = self.alu_bit(!self.regs[b]),
            Neg => self.regs[a] = self.alu_sub(0, self.regs[b]),
            Div => {
                if self.regs[c] == 0 {
                    return self.fault(FaultCode::DivideByZero);
                }
                self.regs[a] = self.alu_bit(self.regs[b] / self.regs[c]);
            }
            Divi => {
                if instr.imm == 0 {
                    return self.fault(FaultCode::DivideByZero);
                }
                self.regs[a] = self.alu_bit(self.regs[b] / instr.imm);
            }
            Rem => {
                if self.regs[c] == 0 {
                    return self.fault(FaultCode::DivideByZero);
                }
                self.regs[a] = self.alu_bit(self.regs[b] % self.regs[c]);
            }
            Remi => {
                if instr.imm == 0 {
                    return self.fault(FaultCode::DivideByZero);
                }
                self.regs[a] = self.alu_bit(self.regs[b] % instr.imm);
            }
            Cmp => {
                let _ = self.alu_sub(self.regs[a], self.regs[b]);
            }
            Cmpi => {
                let _ = self.alu_sub(self.regs[a], instr.imm);
            }
            Load => {
                let addr = self.regs[b].wrapping_add(instr.imm);
                if !self.in_bounds(addr, 4) {
                    return self.fault(FaultCode::Memory);
                }
                self.regs[a] = self.read_word(addr);
            }
            Store => {
                let addr = self.regs[b].wrapping_add(instr.imm);
                if !self.in_bounds(addr, 4) {
                    return self.fault(FaultCode::Memory);
                }
                self.write_word(addr, self.regs[a]);
            }
            Loadb => {
                let addr = self.regs[b].wrapping_add(instr.imm);
                if !self.in_bounds(addr, 1) {
                    return self.fault(FaultCode::Memory);
                }
                self.regs[a] = self.mem[addr as usize] as u32;
            }
            Storeb => {
                let addr = self.regs[b].wrapping_add(instr.imm);
                if !self.in_bounds(addr, 1) {
                    return self.fault(FaultCode::Memory);
                }
                self.mem[addr as usize] = self.regs[a] as u8;
                if addr == self.console_out {
                    self.output.push(self.regs[a] as u8);
                }
            }
            Jmp => self.pc = instr.imm,
            Jz => self.branch(self.flags.zf, instr.imm),
            Jnz => self.branch(!self.flags.zf, instr.imm),
            Jc => self.branch(self.flags.cf, instr.imm),
            Jnc => self.branch(!self.flags.cf, instr.imm),
            Js => self.branch(self.flags.sf, instr.imm),
            Jns => self.branch(!self.flags.sf, instr.imm),
            Jo => self.branch(self.flags.of, instr.imm),
            Jno => self.branch(!self.flags.of, instr.imm),
            Jlt => self.branch(self.flags.sf != self.flags.of, instr.imm),
            Jge => self.branch(self.flags.sf == self.flags.of, instr.imm),
            Jle => self.branch(self.flags.zf || (self.flags.sf != self.flags.of), instr.imm),
            Jgt => self.branch(!self.flags.zf && (self.flags.sf == self.flags.of), instr.imm),
            Push => {
                if self.try_push(self.regs[a]).is_err() {
                    return self.fault(FaultCode::Memory);
                }
            }
            Pop => match self.try_pop() {
                Ok(v) => self.regs[a] = v,
                Err(()) => return self.fault(FaultCode::Memory),
            },
            Call => {
                if self.try_push(self.pc).is_err() {
                    return self.fault(FaultCode::Memory);
                }
                self.pc = instr.imm;
            }
            Ret => match self.try_pop() {
                Ok(v) => self.pc = v,
                Err(()) => return self.fault(FaultCode::Memory),
            },
            Lidt => self.ivt_base = instr.imm,
            Sti => self.flags.ie = true,
            Cli => self.flags.ie = false,
            Iret => {
                let Ok(pc) = self.try_pop() else {
                    return self.fault(FaultCode::Memory);
                };
                let Ok(flags) = self.try_pop() else {
                    return self.fault(FaultCode::Memory);
                };
                self.pc = pc;
                self.flags = Flags::from_u32(flags);
            }
            Trap => {
                if let Some(cb) = syscall {
                    if cb(self, instr.imm) {
                        return StepEvent::Trap(instr.imm);
                    }
                }
                self.regs[6] = instr.imm;
                if self.try_enter_vector(VEC_SYSCALL).is_err() {
                    self.halted = true;
                    return StepEvent::DoubleFault;
                }
                return StepEvent::Trap(instr.imm);
            }
            Halt => {
                self.halted = true;
                return StepEvent::Halted;
            }
            Spget => self.regs[a] = self.sp,
            Spset => self.sp = self.regs[a],
        }

        StepEvent::Ran(instr.op)
    }

    fn branch(&mut self, take: bool, target: u32) {
        if take {
            self.pc = target;
        }
    }

    // ---- ALU with flag semantics ---------------------------------------

    fn set_nz(&mut self, result: u32) {
        self.flags.zf = result == 0;
        self.flags.sf = result & 0x8000_0000 != 0;
    }

    /// Logical/shift result: sets Z and S, clears C and O.
    fn alu_bit(&mut self, result: u32) -> u32 {
        self.set_nz(result);
        self.flags.cf = false;
        self.flags.of = false;
        result
    }

    fn alu_add(&mut self, x: u32, y: u32) -> u32 {
        let (result, carry) = x.overflowing_add(y);
        self.set_nz(result);
        self.flags.cf = carry;
        // Signed overflow: operands share a sign that differs from the result.
        self.flags.of = ((x ^ result) & (y ^ result) & 0x8000_0000) != 0;
        result
    }

    fn alu_sub(&mut self, x: u32, y: u32) -> u32 {
        let (result, borrow) = x.overflowing_sub(y);
        self.set_nz(result);
        self.flags.cf = borrow;
        // Signed overflow on subtraction.
        self.flags.of = ((x ^ y) & (x ^ result) & 0x8000_0000) != 0;
        result
    }

    fn alu_mul(&mut self, x: u32, y: u32) -> u32 {
        let wide = (x as u64).wrapping_mul(y as u64);
        let result = wide as u32;
        self.set_nz(result);
        // Carry/overflow mean the product did not fit in 32 bits.
        let overflowed = wide > u32::MAX as u64;
        self.flags.cf = overflowed;
        self.flags.of = overflowed;
        result
    }
}

impl Default for Cpu {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::isa::Instr;

    fn one(op: Op, a: u8, b: u8, c: u8, imm: u32) -> Instr {
        Instr { op, a, b, c, imm }
    }

    #[test]
    fn halt_stops_execution() {
        let mut cpu = Cpu::new();
        cpu.load_image(0, &one(Op::Halt, 0, 0, 0, 0).encode());
        assert_eq!(cpu.step(None), StepEvent::Halted);
        assert!(cpu.halted);
        // Stepping a halted machine stays halted.
        assert_eq!(cpu.step(None), StepEvent::Halted);
    }

    #[test]
    fn console_mmio_appends_output() {
        let mut cpu = Cpu::new();
        cpu.regs[0] = b'X' as u32;
        cpu.regs[1] = cpu.console_out;
        cpu.load_image(0, &one(Op::Storeb, 0, 1, 0, 0).encode());
        cpu.step(None);
        assert_eq!(cpu.output, vec![b'X']);
    }

    #[test]
    fn bad_opcode_faults() {
        let mut cpu = Cpu::new();
        cpu.mem[0] = 0xEE; // undefined opcode
        assert_eq!(cpu.step(None), StepEvent::Fault(FaultCode::BadOpcode));
    }

    #[test]
    fn syscall_hook_intercepts_trap() {
        use std::cell::Cell;
        use std::rc::Rc;
        let mut cpu = Cpu::new();
        cpu.load_image(0, &one(Op::Trap, 0, 0, 0, 42).encode());
        let seen = Rc::new(Cell::new(0u32));
        let sink = seen.clone();
        let mut hook = move |_c: &mut Cpu, n: u32| {
            sink.set(n);
            true
        };
        let ev = cpu.step(Some(&mut hook));
        assert_eq!(ev, StepEvent::Trap(42));
        assert_eq!(seen.get(), 42);
        assert!(!cpu.halted);
    }
}
