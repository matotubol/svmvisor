import unittest

from build import audit_debug_reset


BODY = '''00001000 <svmvisor_resident_reset_guest_debug>:
1000: xorl %eax, %eax
1002: movq %rax, %dr0
1005: movq %rax, %dr1
1008: movq %rax, %dr2
100b: movq %rax, %dr3
100e: retq
100f: nop
'''


class DebugResetAudit(unittest.TestCase):
    def test_exact_zero_only_helper_passes(self):
        self.assertEqual(audit_debug_reset(BODY)['zeroed_live_registers'], ['DR0', 'DR1', 'DR2', 'DR3'])

    def test_other_debug_write_or_read_and_malformed_helper_fail(self):
        for text in [
            BODY.replace('xorl %eax, %eax', 'movl $1, %eax'),
            BODY.replace('%dr2', '%dr7'),
            BODY.replace('1008: movq %rax, %dr2\n', ''),
            BODY + '1010: movq %rax, %dr0\n',
            BODY + '00002000 <other>:\n2000: movq %rax, %dr0\n',
            BODY + '00002000 <other>:\n2000: movq %dr3, %rax\n',
        ]:
            with self.subTest(text=text), self.assertRaises(RuntimeError):
                audit_debug_reset(text)


if __name__ == '__main__':
    unittest.main()
