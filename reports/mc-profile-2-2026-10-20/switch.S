/* Same x86-64 save/restore and bootstrap as hirsel-rt/src/fiber.rs. */
.text
.globl mc_switch
.type mc_switch,@function
mc_switch:
    push %rbp
    push %rbx
    push %r12
    push %r13
    push %r14
    push %r15
    sub $8, %rsp
    stmxcsr (%rsp)
    fnstcw 4(%rsp)
    mov %rsp, (%rdi)
    mov %rsi, %rsp
    ldmxcsr (%rsp)
    fldcw 4(%rsp)
    add $8, %rsp
    pop %r15
    pop %r14
    pop %r13
    pop %r12
    pop %rbx
    pop %rbp
    ret
.globl mc_start
.type mc_start,@function
mc_start:
    mov %r12, %rdi
    mov %r13, %rsi
    call *%r14
    ud2
.section .note.GNU-stack,"",@progbits
