import unittest

from build import audit_host_fault

ERRORS = {8, 10, 11, 12, 13, 14, 17, 21, 29, 30}
COMMON = '''  10366d:      \tcli
  10366e:      \tclgi
  103671:      \tcld
  103672:      \tmovb\t$0x1, %al
  103674:      \txchgb\t%al, 0x1f9d6(%rip)      # 0x123050 <svmvisor_resident_fault_latched>
  10367a:      \ttestb\t%al, %al
  10367c:      \tjne\t0x1036b9 <svmvisor_resident_fault_stop>
  103682:      \tmovq\t%cr2, %r8
  103686:      \tmovq\t%cr3, %r9
  10368a:      \tmovq\t%rsp, %rsi
  10368d:      \tleaq\t0x1f974(%rip), %rdi     # 0x123008 <svmvisor_resident_fault_record>
  103694:      \tmovl\t$0x7, %ecx
  103699:      \trep\t\tmovsq\t(%rsi), %es:(%rdi)
  10369c:      \tmovq\t%r8, (%rdi)
  10369f:      \tmovq\t%r9, 0x8(%rdi)
  1036a3:      \tleaq\t0x1f95e(%rip), %rdi     # 0x123008 <svmvisor_resident_fault_record>
  1036aa:      \tmovq\t%r8, %rsi
  1036ad:      \tmovq\t%r9, %rdx
  1036b0:      \tandq\t$-0x10, %rsp
  1036b4:      \tcallq\t0x119150 <svmvisor_resident_host_fault>
'''


def gate(vector, fault=None):
    fault = vector if fault is None else fault
    return (f'0000000000100{vector:03x} <svmvisor_resident_irq_{vector}>:\n'
            '  1002d3:      \tclgi\n'
            '  1002d6:      \tcmpl\t$0x1, 0x22d27(%rip)     # 0x123004 <svmvisor_resident_irq_window>\n'
            f'  1002dd:      \tjne\t0x102d0d <svmvisor_resident_fault_{fault}>\n'
            '  1002e3:      \tmovl\t$0x0, 0x22d17(%rip)     # 0x123004 <svmvisor_resident_irq_window>\n'
            f'  1002ed:      \tmovl\t$0x{vector:x}, 0x22d09(%rip)    # 0x123000 <svmvisor_resident_irq_vector>\n'
            '  1002f7:      \tandq\t$-0x201, 0x10(%rsp)     # imm = 0xFDFF\n'
            '  100300:      \tiretq\n\n')


def sx(target='svmvisor_resident_irq_30'):
    return ('0000000000100277 <svmvisor_resident_sx>:\n'
            '  100277:      \tclgi\n'
            '  10027a:      \tcmpq\t$0x1, (%rsp)\n'
            f'  10027f:      \tjne\t0x102cfd <{target}>\n'
            '  100285:      \tlock\n'
            '  100286:      \tincq\t0x7ae33(%rip)           # 0x17b0c0 <svmvisor_resident_init_acks>\n'
            '  10028d:      \taddq\t$0x8, %rsp\n'
            '  100291:      \tiretq\n\n')


def image(gates=None, sx_body=None):
    text = ['0000000000100010 <svmvisor_resident_enter>:\n',
            '  100071:      \tltrw\t%ax\n', '  100078:      \tlidtq\t(%rax)\n\n',
            '000000000010011b <svmvisor_resident_vmrun>:\n', '  10011b:      \tvmrun\n\n',
            sx() if sx_body is None else sx_body]
    for vector in range(16, 256) if gates is None else gates:
        if vector != 18:
            text.append(gate(vector))
    for vector in range(256):
        text.append(f'0000000000102{vector:03x} <svmvisor_resident_fault_{vector}>:\n')
        if vector not in ERRORS:
            text.append('  102c01:      \tpushq\t$0x0\n')
        text.append(f'  102c03:      \tpushq\t$0x{vector:x}\n')
        text.append('  102c05:      \tjmp\t0x10366d <svmvisor_resident_fault_common>\n\n')
    text.append('000000000010366d <svmvisor_resident_fault_common>:\n' + COMMON + '\n')
    return ''.join(text)


class HostFaultAudit(unittest.TestCase):
    def test_window_gates_and_sx_chain_pass(self):
        result = audit_host_fault(image())
        self.assertEqual(result['irq_window_gates'], 239)
        self.assertEqual(result['terminal_only_vectors_below_32'], [*range(16), 18])
        self.assertEqual(result['sx_non_init_path'], ['svmvisor_resident_irq_30', 'svmvisor_resident_fault_30'])

    def test_alignment_padding_after_the_last_gate_is_ignored(self):
        tail = '\tiretq\n'
        padded = gate(255).replace(tail, tail + '  100301:      \tint3\n  100302:      \tnop\n')
        self.assertEqual(audit_host_fault(image().replace(gate(255), padded))['irq_window_gates'], 239)
        # Code after the gate's IRETQ is not padding.
        extra = gate(255).replace(tail, tail + '  100301:      \tclgi\n')
        with self.assertRaises(RuntimeError):
            audit_host_fault(image().replace(gate(255), extra))

    def test_missing_wrong_or_bypassing_gates_fail(self):
        good = image()
        cases = [
            image(gates=[v for v in range(16, 256) if v != 17]),  # vector 17 left as a bare stub
            image(gates=[v for v in range(32, 256)]),              # the old 32-255 set
            good.replace(gate(21), gate(21, fault=22)),           # wrong fallthrough stub
            good.replace(gate(40), gate(40).replace('$0x28', '$0x29')),  # wrong recorded vector
            good.replace(gate(200), gate(200).replace('\tandq\t$-0x201', '\tandq\t$-0x1')),
            good + gate(18),                                      # #MC must stay terminal
            image(sx_body=sx('svmvisor_resident_fault_30')),      # #SX skips the window check
            image(sx_body=sx().replace('\tlock\n', '')),
        ]
        for index, text in enumerate(cases):
            with self.subTest(case=index), self.assertRaises(RuntimeError):
                audit_host_fault(text)


if __name__ == '__main__':
    unittest.main()
