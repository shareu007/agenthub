"""Regression coverage for PTY EOF handling on Linux and macOS."""
import errno
import unittest
from unittest.mock import patch
from verify_selector import drain_output, read_chunk


class PtyReadTests(unittest.TestCase):
    def test_hangup_readiness_does_not_spin_after_empty_read(self):
        output = bytearray()
        with patch('verify_selector.select.select', return_value=([7], [], [])), \
             patch('verify_selector.os.read', side_effect=[b'pending output', b'']) as read:
            drain_output(7, output)
        self.assertEqual(output, b'pending output')
        self.assertEqual(read.call_count, 2)

    def test_linux_eio_hangup_ends_drain(self):
        with patch('verify_selector.select.select', return_value=([7], [], [])), \
             patch('verify_selector.os.read', side_effect=OSError(errno.EIO, 'hangup')) as read:
            drain_output(7, bytearray())
        self.assertEqual(read.call_count, 1)

    def test_unexpected_descriptor_error_is_not_hidden(self):
        with patch('verify_selector.os.read', side_effect=OSError(errno.EBADF, 'bad descriptor')):
            with self.assertRaises(OSError):
                read_chunk(7)


if __name__ == '__main__':
    unittest.main()
