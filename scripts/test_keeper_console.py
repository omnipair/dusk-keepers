"""Offline terminal presentation checks. Never load real keys or contact RPC."""
import contextlib
import importlib.util
import io
import json
import os
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

from keeper_console import HumanLogs

POSITION = '5GFk5KrtaEt3gZC8g3uLp4fJm566dy529JCWzqaFMsnZ'
MINT = 'omfgRBnxHsNJh6YeGbGAmWenNkenzsXyBXm3WDhmeta'
QUOTE = 'METAwkXcqyXKy1AtsSgJ8JiUHwGCafnZL38n3vYmeta'
SIGNATURE = '3' * 88


def position(state='HEALTHY', health='1.064848'):
    return [
        f'position_health kind=LeveragePosition position={POSITION}',
        '  owner=owner market=market scope="supported market"',
        '  debt=263.607504 BASE debt_raw=263607504 collateral=13.739348 QUOTE collateral_raw=13739348 collateral_value=332.113180 BASE debt_shares=263585092',
        f'  state={state} health_factor={health} liquidation_at=1.000000 ltv_pct=79.3727 liquidation_ltv_pct=84.52 maintenance_pct=15.48 eligible=false',
        f'  debt_mint={MINT} collateral_mint={QUOTE} snapshot_slot=123 preview_slot=124',
    ]


def render(formatter, lines):
    return '\n'.join(message for line in lines for message, _ in formatter.feed(line))


class ConsoleTests(unittest.TestCase):
    def test_health_keeps_amounts_and_removes_machine_fields(self):
        output = render(HumanLogs(), position())
        self.assertIn('Healthy', output)
        self.assertIn('Health 1.064848', output)
        self.assertIn('Debt 263.607504 base', output)
        self.assertIn('Collateral 13.739348 quote', output)
        self.assertIn('Collateral value 332.11318 base', output)
        self.assertNotIn('=', output)
        self.assertNotIn('snapshot', output)

    def test_healthy_repeats_are_throttled_but_eligibility_changes_are_immediate(self):
        now = [0]
        formatter = HumanLogs(clock=lambda: now[0])
        self.assertTrue(render(formatter, position()))
        now[0] = 5
        self.assertFalse(render(formatter, position(health='1.064847')))
        self.assertIn('Ready for liquidation', render(formatter, position('LIQUIDATABLE', '1.000000')))
        now[0] = 66
        self.assertTrue(render(formatter, position('LIQUIDATABLE', '1.000000')))

    def test_unavailable_and_debt_free_positions_have_no_invented_ratio(self):
        formatter = HumanLogs()
        output = render(formatter, [f'position_health position={POSITION} state=UNAVAILABLE reason="market is missing or unsupported"'])
        self.assertIn('Cannot check position', output)
        self.assertIn('market is missing', output)
        self.assertIn('Health n/a', render(formatter, position('DEBT_FREE', 'n/a')))

    def test_nested_program_error_is_explained_without_json(self):
        detail = 'repay=3.750017 raw=3750017 full=false; ' + json.dumps({
            'InstructionError': [2, {'Custom': 6138}],
            'logs': ['Error Code: LiquidationDoesNotImproveHealth. Error Message: Partial liquidation does not improve health after costs.'],
        })
        line = f'liquidation_detail position={POSITION} stage=ordinary_preview_rejected detail={json.dumps(detail)}'
        output = render(HumanLogs(), [line])
        self.assertIn('would not improve', output)
        self.assertNotIn('InstructionError', output)
        self.assertNotIn('{', output)

    def test_simulation_never_claims_actual_payment_or_reward(self):
        detail = f'SIMULATED wallet_payment=1.000001 collateral_received=2.000001 debt_mint={MINT} collateral_mint={QUOTE}'
        line = f'liquidation_detail position={POSITION} stage=inventory_simulation detail={json.dumps(detail)}'
        output = render(HumanLogs(), [line])
        self.assertIn('Simulation only', output)
        self.assertIn('would pay 1.000001', output)
        self.assertIn('No tokens spent yet', output)

    def test_finalized_receipt_keeps_payments_rewards_fees_and_both_swap_legs(self):
        output = render(HumanLogs(), [
            '=== FINALIZED SUCCESS | emergency ===',
            '  Signature: ' + SIGNATURE,
            '  SOL fee paid: 0.000005000 SOL (5000 lamports; includes priority fee)',
            f'  NET PAID -1.000001 | 2.000001 -> 1.000000 | mint={MINT}',
            f'  NET RECEIVED +2.000001 | 0.000000 -> 2.000001 | mint={QUOTE}',
            '  SWAP EXECUTED: Dusk',
            f'    IN  1.000001 mint={MINT} BASE',
            f'    OUT 2.000001 mint={QUOTE} QUOTE',
            f'    REWARD RECEIVED: 0.000001 mint={QUOTE} QUOTE | recipient={POSITION}',
            f'    [2.1] TRANSFER 1.000001 mint={MINT}',
            f'      keeper ({POSITION}) -> reserve_vault ({MINT})',
            '=== END RECEIPT ===',
        ])
        for value in ['Confirmed and finalized', 'Paid 1.000001', 'Received 2.000001', 'SOL fee paid: 0.000005000',
                      'Swap input: 1.000001', 'Swap output: 2.000001', 'Reward received: 0.000001',
                      'Transfer 1.000001', 'reserve vault', 'https://solscan.io/tx/' + SIGNATURE]:
            self.assertIn(value, output)
        self.assertNotIn('=', output)
        self.assertNotIn('[2.1]', output)

    def test_failed_receipt_is_not_reported_as_success_or_reward(self):
        output = render(HumanLogs(), ['=== FINALIZED FAILED | flash ===',
            'Error: {"InstructionError":[2,{"Custom":6138}]}',
            'SOL fee paid: 0.000005000 SOL',
            'Swap events and attempted transfers were rolled back; they are not reported as payments or rewards.',
            '=== END RECEIPT ==='])
        self.assertIn('Failed on-chain', output)
        self.assertIn('would not improve', output)
        self.assertIn('rolled back', output)
        self.assertNotIn('Confirmed and finalized', output)
        self.assertNotIn('Reward received:', output)

    def test_pending_submission_stays_pending_until_receipt_and_is_not_downgraded(self):
        formatter = HumanLogs()
        sent = f'keeper executed position={POSITION} debt_mint={MINT} signature={SIGNATURE}'
        self.assertIn('waiting for final confirmation', render(formatter, [sent]))
        self.assertFalse(render(formatter, [sent]))
        render(formatter, ['=== FINALIZED SUCCESS | LeverageEntryOrder ===', 'Signature: ' + SIGNATURE, '=== END RECEIPT ==='])
        self.assertFalse(render(formatter, [sent]))

    def test_status_errors_are_visible_and_speak_in_sol(self):
        formatter = HumanLogs()
        messages = formatter.health({'status': 'degraded', 'reason': 'signer holds 10000000 lamports, below the 50000000 floor'})
        self.assertIn('0.01 SOL', messages[0][0])
        self.assertIn('0.05 SOL', messages[0][0])
        self.assertIn('recovered', formatter.health({'status': 'ready'})[0][0])

    def test_closed_position_order_is_a_readable_skip_not_a_worker_error(self):
        formatter = HumanLogs()
        line = f'order_skipped order={MINT} position={POSITION} reason=position_closed'
        output = render(formatter, [line])
        self.assertIn('its position', output)
        self.assertIn('no longer exists', output)
        self.assertIn('Continuing with other orders', output)
        self.assertNotIn('=', output)
        self.assertNotIn('Error', output)
        self.assertEqual(render(formatter, [line]), '')

    def test_unknown_diagnostics_never_dump_raw_data(self):
        output = render(HumanLogs(), ['new_diagnostic payload={"very":"technical"}', 'Error: {"unknown":{"data":1}}'])
        self.assertIn('saved', output)
        self.assertIn('Error:', output)
        self.assertNotIn('{', output)
        self.assertNotIn('payload=', output)
        self.assertNotIn('private.rs', render(HumanLogs(), ["thread 'main' panicked at private.rs:42"]))

    def test_actual_terminal_pipe_writes_raw_file_and_only_human_console(self):
        spec = importlib.util.spec_from_file_location('launcher', Path(__file__).with_name('local-mainnet.py'))
        launcher = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(launcher)
        raw = '\n'.join(position()) + '\n'
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / 'logs').mkdir()
            fake = root / 'fake-keeper'
            fake.write_text('#!' + sys.executable + '\nimport sys\nsys.stdout.write(' + repr(raw) + ')\nsys.exit(1)\n')
            fake.chmod(0o700)
            output = io.StringIO()
            with patch.object(launcher, 'ROOT', root), patch.object(launcher, 'BINARY', fake), contextlib.redirect_stdout(output):
                with self.assertRaisesRegex(RuntimeError, 'leverage exited'):
                    launcher.terminal({'leverage': {'PATH': os.environ.get('PATH', ''), 'KEEPER_LOG_COLOR': 'never'}})
            self.assertIn('Healthy', output.getvalue())
            self.assertNotIn('position_health', output.getvalue())
            self.assertNotIn('debt_raw=', output.getvalue())
            self.assertIn('position_health', (root / 'logs/leverage.log').read_text())
            self.assertIn('debt_raw=263607504', (root / 'logs/leverage.log').read_text())

    def test_preflight_failure_is_human_and_retains_original_diagnostics(self):
        spec = importlib.util.spec_from_file_location('launcher', Path(__file__).with_name('local-mainnet.py'))
        launcher = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(launcher)
        raw = 'Error: {"InstructionError":[2,{"Custom":6138}]}\n'
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / 'logs').mkdir()
            fake = root / 'fake-check'
            fake.write_text('#!' + sys.executable + '\nimport sys\nsys.stderr.write(' + repr(raw) + ')\nsys.exit(1)\n')
            fake.chmod(0o700)
            output = io.StringIO()
            with patch.object(launcher, 'ROOT', root), patch.object(launcher, 'BINARY', fake), contextlib.redirect_stdout(output):
                with self.assertRaisesRegex(RuntimeError, 'startup checks failed'):
                    launcher.preflight('leverage', {'KEEPER_LOG_COLOR': 'never'})
            self.assertIn('would not improve', output.getvalue())
            self.assertNotIn('InstructionError', output.getvalue())
            self.assertIn('InstructionError', (root / 'logs/leverage.log').read_text())


if __name__ == '__main__':
    unittest.main()
