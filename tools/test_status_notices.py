#!/usr/bin/env python3
"""Run a controllable 20103 notice test server, one notice per client login."""
from __future__ import annotations

import argparse
import struct
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
KINDS = {
    1: 'Unknown non-dialog action', 2: 'Unknown non-dialog action',
    3: 'Login failed: error 3 (observed)', 4: 'Login failed: error 4',
    5: 'Login failed: error 5', 6: 'Login failed: error 6',
    7: 'Resource descriptor update path', 8: 'Login failed with text',
    9: 'Custom text / banned dialog', 10: 'Login mode 2 with text and integer',
    11: 'Resource update done path', 12: 'Login failed: error 12',
    13: 'Login failed: error 13', 14: 'Login failed: error 14',
    15: 'Unknown non-dialog action', 16: 'Unknown non-dialog action',
    17: 'Account deletion warning (positive tail 1)',
    18: 'Account deletion branch (tail 2: -3, -2, or other)',
}


def vint(value: int) -> bytes:
    if not -(1 << 31) <= value < (1 << 31):
        raise ValueError('VInt must fit a signed 32-bit integer')
    rest = value >> 6
    out = bytearray([(value & 0x3f) | (0x40 if value < 0 else 0)
                     | (0x80 if rest not in (0, -1) else 0)])
    while rest not in (0, -1):
        next_rest = rest >> 7
        out.append((rest & 0x7f) | (0x80 if next_rest not in (0, -1) else 0))
        rest = next_rest
    return bytes(out)


def string(value: str | None) -> bytes:
    if value is None:
        return struct.pack('>i', -1)
    data = value.encode('utf-8')
    if len(data) > 900000:
        raise ValueError('string exceeds client limit of 900000 bytes')
    return struct.pack('>i', len(data)) + data


def boolean(value: str) -> bool:
    if value.lower() in ('true', '1', 'yes', 'on'):
        return True
    if value.lower() in ('false', '0', 'no', 'off'):
        return False
    raise argparse.ArgumentTypeError('expected true or false')


def body(kind: int, args: argparse.Namespace) -> bytes:
    tail1 = args.tail1 if args.tail1 is not None else (60 if kind == 17 else 0)
    tail2 = args.tail2 if args.tail2 is not None else (-2 if kind == 18 else 0)
    return (struct.pack('>i', kind) + string(args.text) + string(args.message)
            + struct.pack('>i', args.extra_int) + bytes([args.flag])
            + string(args.extra_text) + vint(tail1) + vint(tail2))


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, epilog=(
        'Each selection starts the server. Reconnect the client, observe the notice, '
        'then press Enter to stop the server and choose another. '
        'Pass additional server options after --.'))
    parser.add_argument('kind', nargs='?', type=int, choices=range(1, 19),
                        help='test one kind; omit for interactive selection')
    parser.add_argument('--config', type=Path, default=ROOT / 'config.toml', help='server TOML configuration')
    parser.add_argument('--binary', type=Path, help='existing hd-server binary (default: build with cargo)')
    parser.add_argument('--message', default='Test notice from the custom server', help='notice message (+0x40)')
    parser.add_argument('--text', help='first optional string (+0x3c); default null')
    parser.add_argument('--extra-int', type=int, default=0, help='signed BE32 field (+0x38)')
    parser.add_argument('--flag', nargs='?', const=True, default=False, type=boolean,
                        metavar='BOOL', help='set +0x44 (kind 9 support dialog variant); accepts true or false')
    parser.add_argument('--extra-text', help='optional string (+0x48); default null')
    parser.add_argument('--tail1', type=int, help='signed VInt +0x4c (default 60 for kind 17, otherwise 0)')
    parser.add_argument('--tail2', type=int, help='signed VInt +0x50 (default -2 for kind 18, otherwise 0)')
    parser.add_argument('--list', action='store_true', help='list the 18 kinds and exit')
    parser.add_argument('--body-hex', action='store_true', help='print the selected body and exit without starting a server')
    args, server_args = parser.parse_known_args()
    if server_args and server_args[0] == '--':
        server_args = server_args[1:]
    elif server_args:
        parser.error('additional server options must follow --')
    if args.list:
        for number, description in KINDS.items():
            print(f'{number:2}: {description}')
        return 0
    if args.body_hex:
        if args.kind is None:
            parser.error('--body-hex requires a kind')
        try:
            print(body(args.kind, args).hex())
        except (ValueError, struct.error) as exc:
            parser.error(str(exc))
        return 0
    if args.binary is None:
        print('Building hd-server...', flush=True)
        subprocess.run(['cargo', 'build', '--bin', 'hd-server'], cwd=ROOT, check=True)
        binary = ROOT / 'target/debug/hd-server'
    else:
        binary = args.binary.resolve()
    print('20103 changes the client to an error state. Reconnect for each test.', flush=True)
    while True:
        if args.kind is None:
            for number, description in KINDS.items():
                print(f'{number:2}: {description}')
            selected = input('Kind 1–18, or q to quit: ').strip().lower()
            if selected in ('q', 'quit', ''):
                return 0
            try:
                kind = int(selected)
                if kind not in KINDS:
                    raise ValueError
            except ValueError:
                print('Choose a number from 1 to 18.')
                continue
        else:
            kind = args.kind
        try:
            payload = body(kind, args)
        except (ValueError, struct.error) as exc:
            parser.error(str(exc))
        with tempfile.TemporaryDirectory(prefix='hd-notice-') as directory:
            path = Path(directory) / 'notice.bin'
            path.write_bytes(payload)
            command = [str(binary), '--config', str(args.config.resolve()), *server_args,
                       '--test-status-notice', str(path), '--after-ready-once']
            print(f'\nTesting kind {kind}: {KINDS[kind]}\nBody: {payload.hex()}\n'
                  'Reconnect the client; press Enter to stop this server.', flush=True)
            process = subprocess.Popen(command, cwd=ROOT, stdin=subprocess.DEVNULL)
            try:
                input()
            finally:
                if process.poll() is None:
                    process.terminate()
                    try:
                        process.wait(timeout=5)
                    except subprocess.TimeoutExpired:
                        process.kill()
                        process.wait()
            if process.returncode not in (0, -15):
                print(f'Server exited with status {process.returncode}.', file=sys.stderr)
        if args.kind is not None:
            return 0 if process.returncode in (0, -15) else 1


if __name__ == '__main__':
    try:
        raise SystemExit(main())
    except (KeyboardInterrupt, EOFError):
        print('\nStopped.')
        raise SystemExit(130)
    except (OSError, subprocess.CalledProcessError) as exc:
        print(f'Could not run notice test: {exc}', file=sys.stderr)
        raise SystemExit(1)
