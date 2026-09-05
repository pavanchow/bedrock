//! Gate 1b: the assembler produces expected bytes for known snippets and the
//! disassemble/assemble round-trip is stable.

use bedrock::assembler::assemble;
use bedrock::disasm::disasm_source;
use bedrock::isa::{Instr, Op};

#[test]
fn known_bytes_movi() {
    // movi r1, 0x2A  ->  opcode 0x02, a=1, imm=0x2A little-endian.
    let asm = assemble("movi r1, 0x2A").unwrap();
    assert_eq!(asm.code, vec![0x02, 0x01, 0x00, 0x00, 0x2A, 0x00, 0x00, 0x00]);
}

#[test]
fn known_bytes_add() {
    // add r0, r1, r2 -> opcode 0x03, a=0,b=1,c=2.
    let asm = assemble("add r0, r1, r2").unwrap();
    assert_eq!(asm.code, vec![0x03, 0x00, 0x01, 0x02, 0x00, 0x00, 0x00, 0x00]);
}

#[test]
fn known_bytes_load_disp() {
    // load r3, [r2 + 4] -> opcode 0x17, a=3, b=2, imm=4.
    let asm = assemble("load r3, [r2 + 4]").unwrap();
    assert_eq!(asm.code, vec![0x17, 0x03, 0x02, 0x00, 0x04, 0x00, 0x00, 0x00]);
}

#[test]
fn labels_resolve_to_addresses() {
    let src = "\
        start:  movi r0, 0\n\
        loop:   addi r0, r0, 1\n\
                cmpi r0, 3\n\
                jlt loop\n\
                jmp start\n";
    let asm = assemble(src).unwrap();
    assert_eq!(asm.labels["start"], 0);
    assert_eq!(asm.labels["loop"], 8);
    // jlt loop targets address 8.
    let jlt = Instr::decode(chunk(&asm.code, 24)).unwrap();
    assert_eq!(jlt.op, Op::Jlt);
    assert_eq!(jlt.imm, 8);
    // jmp start targets address 0.
    let jmp = Instr::decode(chunk(&asm.code, 32)).unwrap();
    assert_eq!(jmp.op, Op::Jmp);
    assert_eq!(jmp.imm, 0);
}

#[test]
fn directives_word_byte_space() {
    let src = "\
        .word 0x11223344\n\
        .byte 1, 2, 3, 4\n\
        .space 4\n";
    let asm = assemble(src).unwrap();
    assert_eq!(&asm.code[0..4], &[0x44, 0x33, 0x22, 0x11]);
    assert_eq!(&asm.code[4..8], &[1, 2, 3, 4]);
    assert_eq!(&asm.code[8..12], &[0, 0, 0, 0]);
}

#[test]
fn equ_constants() {
    let src = "\
        .equ ANSWER 42\n\
        movi r0, ANSWER\n";
    let asm = assemble(src).unwrap();
    let i = Instr::decode(chunk(&asm.code, 0)).unwrap();
    assert_eq!(i.imm, 42);
}

/// The core round-trip property: assembling the disassembly of an image
/// reproduces the exact bytes.
#[test]
fn disasm_assemble_roundtrip_is_stable() {
    let src = "\
        movi r0, 10\n\
        movi r1, 20\n\
        add r2, r0, r1\n\
        addi r2, r2, 5\n\
        sub r3, r2, r0\n\
        and r4, r0, r1\n\
        shli r5, r0, 2\n\
        not r6, r0\n\
        neg r7, r1\n\
        cmp r0, r1\n\
        cmpi r0, 7\n\
        store r0, [r1 + 8]\n\
        load r2, [r1 - 4]\n\
        loadb r3, [r1]\n\
        push r0\n\
        pop r1\n\
        jmp 128\n\
        jz 256\n\
        jnz 64\n\
        call 512\n\
        ret\n\
        lidt 4096\n\
        sti\n\
        cli\n\
        trap 1\n\
        iret\n\
        spget r0\n\
        spset r0\n\
        halt\n";
    let first = assemble(src).unwrap();
    let text = disasm_source(&first.code, first.origin);
    let second = assemble(&text).unwrap();
    assert_eq!(
        first.code, second.code,
        "round-trip changed the bytes\n--- disassembly ---\n{text}"
    );

    // And disassembling twice is idempotent at the text level.
    let text2 = disasm_source(&second.code, second.origin);
    assert_eq!(text, text2, "disassembly is not stable");
}

fn chunk(code: &[u8], off: usize) -> [u8; 8] {
    let mut b = [0u8; 8];
    b.copy_from_slice(&code[off..off + 8]);
    b
}
