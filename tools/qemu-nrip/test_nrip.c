#include <assert.h>
#include <stdbool.h>
#include <stdint.h>
#include <stddef.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <setjmp.h>
#include <inttypes.h>
#define QEMU_PACKED __attribute__((packed))
#include "svm.h"
#include "defines.inc"
#define FEAT_SVM 0
#define EXCP03_INT3 3
#define EXCP04_INTO 4
#define EXCP05_BOUND 5
#define EXCP02_NMI 2
#define EXCP_VMEXIT 0x100
#define INSN_START_WORDS 3
#define QEMU_BUILD_BUG_ON(x) _Static_assert(!(x), #x)
#define qemu_log_mask(...) ((void)0)
#define GETPC() 1

typedef struct CPUState { int exception_index; } CPUState;
typedef struct CPUX86State {
    CPUState cpu;
    uint64_t eip, vm_vmcb, exception_next_eip;
    uint32_t hflags, features[1];
    int old_exception, error_code, exception_is_int;
} CPUX86State;
static CPUState *env_cpu(CPUX86State *env) { return &env->cpu; }
static struct vmcb memory;
static uint64_t info, restored_rip;
static bool unwind_ok;
static unsigned restores, unwinds, checks;
static jmp_buf escape;
static bool cpu_unwind_state_data(CPUState *cs, uintptr_t ra, uint64_t *data)
{ unwinds++; data[2]=info; return unwind_ok; }
static void cpu_restore_state(CPUState *cs, uintptr_t ra)
{ restores++; ((CPUX86State *)cs)->eip=restored_rip; }
static void x86_stq_phys(CPUState *cs, uint64_t address, uint64_t value)
{ assert(address+8<=sizeof memory); memcpy((char *)&memory+address,&value,8); }
static uint64_t x86_ldq_phys(CPUState *cs, uint64_t address)
{ uint64_t value; assert(address+8<=sizeof memory); memcpy(&value,(char *)&memory+address,8); return value; }
static void cpu_loop_exit(CPUState *cs) { longjmp(escape,1); }
#include "actual.inc"
#define CHECK(x) do { checks++; assert(x); } while (0)

static void vmexit_case(unsigned code, unsigned length, unsigned origin,
                        unsigned flags, uint64_t rip, bool feature,
                        uintptr_t ra, bool decoded, uint64_t expected)
{
    static CPUX86State env; memset(&env,0,sizeof env);
    env.eip=0xbadcafe; env.hflags=flags;
    env.features[0]=feature?CPUID_SVM_NRIPSAVE:0;
    memory.control.next_rip=0xdeadbeef;
    restored_rip=rip; info=length|((uint64_t)origin<<8); unwind_ok=decoded;
    restores=unwinds=0;
    if (!setjmp(escape)) { cpu_vmexit(&env,code,0x1234,ra); abort(); }
    CHECK(memory.control.next_rip==expected);
    CHECK(memory.control.exit_code==code);
    CHECK(memory.control.exit_info_1==0x1234);
    CHECK(env.cpu.exception_index==EXCP_VMEXIT);
    CHECK(restores==1 && unwinds==(ra!=0));
}

int main(void)
{
    const unsigned codes[]={SVM_EXIT_READ_CR0,SVM_EXIT_WRITE_CR3,
        SVM_EXIT_READ_DR0,SVM_EXIT_WRITE_DR7,SVM_EXIT_CR0_SEL_WRITE,
        SVM_EXIT_CPUID,SVM_EXIT_MSR,SVM_EXIT_IOIO,SVM_EXIT_SWINT,
        SVM_EXIT_IRET,SVM_EXIT_VMRUN,SVM_EXIT_VMMCALL,SVM_EXIT_RDTSCP,
        SVM_EXIT_ICEBP,SVM_EXIT_MWAIT,SVM_EXIT_XSETBV};
    const unsigned modes[]={0,HF_CS32_MASK,HF_CS64_MASK};
    for(unsigned c=0;c<sizeof codes/sizeof codes[0];c++)
        for(unsigned m=0;m<3;m++) for(unsigned n=1;n<=15;n++)
            vmexit_case(codes[c],n,0,modes[m],0x1230,true,1,true,0x1230+n);
    vmexit_case(SVM_EXIT_CPUID,3,0,0,0xfffe,true,1,true,1);
    vmexit_case(SVM_EXIT_CPUID,5,0,HF_CS32_MASK,0xfffffffe,true,1,true,3);
    vmexit_case(SVM_EXIT_CPUID,5,0,HF_CS64_MASK,0xfffffffe,true,1,true,0x100000003);
    for(unsigned n=3;n<=5;n++) {
        vmexit_case(SVM_EXIT_EXCP_BASE+n,3,n,HF_CS32_MASK,0x1230,true,1,true,0x1233);
        vmexit_case(SVM_EXIT_EXCP_BASE+n,3,0,HF_CS32_MASK,0x1230,true,1,true,0);
    }
    const unsigned zero[]={SVM_EXIT_INTR,SVM_EXIT_NMI,SVM_EXIT_VINTR,
        SVM_EXIT_EXCP_BASE+1,SVM_EXIT_EXCP_BASE+13,SVM_EXIT_EXCP_BASE+14,
        SVM_EXIT_NPF,SVM_EXIT_SHUTDOWN,SVM_EXIT_ERR,SVM_EXIT_TASK_SWITCH};
    for(unsigned c=0;c<sizeof zero/sizeof zero[0];c++)
        vmexit_case(zero[c],5,3,HF_CS64_MASK,0x1230,true,1,true,0);
    vmexit_case(SVM_EXIT_CPUID,3,0,HF_CS64_MASK,0x1230,false,1,true,0);
    vmexit_case(SVM_EXIT_CPUID,3,0,HF_CS64_MASK,0x1230,true,0,true,0);
    vmexit_case(SVM_EXIT_CPUID,3,0,HF_CS64_MASK,0x1230,true,1,false,0);
    vmexit_case(SVM_EXIT_CPUID,0,0,HF_CS64_MASK,0x1230,true,1,true,0);
    vmexit_case(SVM_EXIT_CPUID,16,0,HF_CS64_MASK,0x1230,true,1,true,0);
    for(unsigned feature=0;feature<=1;feature++) for(unsigned vector=3;vector<=4;vector++) {
        static CPUX86State env; memset(&env,0,sizeof env); env.eip=0x1111; memory.control.next_rip=0x2222;
        env.features[0]=feature?CPUID_SVM_NRIPSAVE:0;
        if(!setjmp(escape)) { inject(&env,SVM_EVTINJ_TYPE_EXEPT,vector); abort(); }
        CHECK(env.exception_is_int==1);
        CHECK(env.exception_next_eip==(feature?0x2222:0x1111));
        CHECK(env.cpu.exception_index==vector);
    }
    for(unsigned feature=0;feature<=1;feature++) {
        static CPUX86State env; memset(&env,0,sizeof env); env.eip=0x3333; memory.control.next_rip=0x4444;
        env.features[0]=feature?CPUID_SVM_NRIPSAVE:0;
        if(!setjmp(escape)) { inject(&env,SVM_EVTINJ_TYPE_SOFT,0x80); abort(); }
        CHECK(env.exception_is_int==1);
        CHECK(env.exception_next_eip==(feature?0x4444:0x3333));
        if(!setjmp(escape)) { inject(&env,SVM_EVTINJ_TYPE_EXEPT,13); abort(); }
        CHECK(env.exception_is_int==0 && env.exception_next_eip==UINT64_MAX);
    }
    printf("PASS %u actual-C NRIP publication/injection assertions; CPU/unwind/memory/loop stubs, no TCG execution proof\n",checks);
    return 0;
}
