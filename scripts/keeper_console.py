"""Presentation only: turn native diagnostics into plain-language terminal output.

The launcher always saves the original line before calling this formatter. No
RPC calls, amounts used for execution, or transaction decisions live here.
"""
import json
import re
import time
from decimal import Decimal, InvalidOperation

ANSI = re.compile(r'\x1b\[[0-9;]*m')
ADDRESS = re.compile(r'\b[1-9A-HJ-NP-Za-km-z]{32,88}\b')
FIELD = re.compile(r'\b([a-z][a-z0-9_]*)=("(?:\\.|[^"\\])*"|[^\s;]+)')
LABELS = {'lending': 'Lending', 'leverage': 'Leverage', 'hlp-recovery': 'hLP recovery',
          'auction-arbitrage': 'Auctions', 'lifecycle': 'Lifecycle', 'sentinel': 'Monitor'}
ACTIONS = {'LeverageOrder': 'TP/SL order', 'LeverageEntryOrder': 'Entry order',
           'HlpOrder': 'hLP order', 'flash': 'Ordinary liquidation',
           'flash_full': 'Full liquidation', 'emergency': 'Emergency liquidation',
           'emergency_full': 'Full emergency liquidation', 'prepare_accounts': 'Wallet account setup'}


def short(value):
    return value[:6] + '…' + value[-5:] if len(value) > 16 else value


def fields(text):
    result = {}
    for match in FIELD.finditer(text):
        value = match[2]
        if value.startswith('"'):
            try:
                value = json.loads(value)
            except ValueError:
                value = value[1:-1]
        result[match[1]] = value
    return result


def amount(value):
    # Only trim insignificant zeroes; never turn token amounts into floats.
    if re.fullmatch(r'[+-]?\d+\.\d+', value):
        return value.rstrip('0').rstrip('.')
    return value


def readable(text):
    text = ANSI.sub('', str(text)).strip()
    text = ADDRESS.sub(lambda m: short(m[0]), text)
    text = text.replace('_', ' ').replace(' -> ', ' → ')
    return text


def reason(raw):
    text = ANSI.sub('', str(raw)).strip()
    if text.startswith('Error: '):
        text = text[7:]
    if text.startswith('"'):
        try:
            text = json.loads(text)
        except ValueError:
            pass
    known = [
        ('LiquidationDoesNotImproveHealth', 'Partial liquidation would not improve the position’s health after fees.'),
        ('InsufficientOutputAmount', 'The swap would return less than the required minimum.'),
        ('InsufficientFundsForFee', 'The wallet needs more SOL to pay transaction fees.'),
        ('insufficient debt-token inventory', 'The wallet does not have enough debt tokens for this repayment.'),
        ('no spendable debt-token inventory', 'No debt tokens are available in the wallet for repayment.'),
        ('clock and RPC bank do not match', 'The RPC returned inconsistent account data. A fresh snapshot is needed.'),
        ('ordinary_fallback:', 'Refreshing the position before trying full liquidation.'),
        ('emergency_eligibility:', 'Refreshing the position to check emergency liquidation eligibility.'),
        ('emergency_unavailable:', 'The position is not eligible for emergency liquidation.'),
        ('partial_simulation_rejected:', 'Partial liquidation failed its simulation. Refreshing before the next attempt.'),
        ('partial_revalidation_rejected:', 'The partial liquidation is no longer valid. Refreshing the position.'),
        ('deployment changed', 'The deployed program has changed. Update the keeper’s deployment capture before restarting.'),
        ('deployment account identity changed', 'The deployed program no longer matches this keeper’s configuration.'),
        ('last completed scan is stale', 'The last scan is taking longer than expected.'),
        ('execution is not configured', 'Execution settings are missing. Check the keeper configuration.'),
        ('state or fee epoch changed', 'The position or token fees changed. A new quote is needed.'),
        ('fee budget', 'The transaction would exceed the configured SOL fee budget.'),
        ('SOL reserve', 'The transaction would leave too little SOL in the wallet.'),
        ('NotFrozen', 'This protocol release has not been enabled for live execution.'),
    ]
    for needle, message in known:
        if needle in text:
            return message
    balance = re.search(r'signer holds (\d+) lamports, below the (\d+) floor', text)
    if balance:
        actual, required = (amount(format(Decimal(x) / Decimal(10**9), 'f')) for x in balance.groups())
        return f'The wallet has {actual} SOL; at least {required} SOL is required.'
    anchor = re.search(r'Error Message: ([^"\n\\|]+)', text)
    if anchor:
        return readable(anchor[1]).rstrip('.') + '.'
    custom = re.search(r'(?:"Custom"\s*:\s*|Error Number:\s*)(\d+)', text)
    if custom:
        if custom[1] == '6138':
            return 'Partial liquidation would not improve the position’s health after fees.'
        if custom[1] == '6032':
            return 'The swap would return less than the required minimum.'
        return 'The on-chain program rejected the transaction (error ' + custom[1] + '). See the detailed log.'
    if any(word in text.lower() for word in ('timeout', 'timed out', 'connection refused', 'transport failed')):
        return 'Could not reach a required service. Check the RPC and database connection; details are in the log.'
    if any(char in text for char in '{}[]') or re.search(r'\b\w+=', text) or len(text) > 240:
        return 'This attempt could not complete. The detailed reason is saved in the log.'
    return readable(text).rstrip('.') + '.' if text else 'Details are saved in the log.'


class HumanLogs:
    def __init__(self, clock=time.monotonic):
        self.clock = clock
        self.position = None
        self.seen = {}
        self.positions = {}
        self.in_receipt = False
        self.receipt_failed = False
        self.finalized = set()
        self.last_health = None

    def once(self, key, message, color='36', interval=60):
        now = self.clock()
        previous = self.seen.get(key)
        if previous and previous[0] == message and now - previous[1] < interval:
            return []
        self.seen[key] = (message, now)
        # Bound presentation state even on very long runs.
        if len(self.seen) > 4096:
            self.seen.pop(next(iter(self.seen)))
        return [(message, color)]

    def feed(self, line):
        text = ANSI.sub('', line).strip()
        if not text:
            return []
        if text.startswith('=== ') and 'FINALIZED ' in text:
            self.in_receipt = True
            self.receipt_failed = 'FINALIZED FAILED' in text
            action = text.split('|', 1)[-1].strip(' =')
            action = ACTIONS.get(action, readable(action).capitalize())
            state = 'Failed on-chain' if self.receipt_failed else 'Confirmed and finalized'
            return [(state + ': ' + action + '.', '31' if self.receipt_failed else '32')]
        if self.in_receipt:
            return self.receipt(text)
        data = fields(text)
        if text.startswith('position_scan '):
            count = data.get('discovered', '?')
            return self.once('scan', f'Checking {count} open position' + ('' if count == '1' else 's') + '.', '2')
        if text.startswith('position_health '):
            if data.get('state') == 'UNAVAILABLE':
                return self.once(('unavailable', data.get('position')), 'Cannot check position ' +
                                 short(data.get('position', '?')) + ': ' + reason(data.get('reason')), '33')
            self.position = data
            return []
        if self.position and text.startswith(('owner=', 'debt=', 'state=', 'debt_mint=')):
            self.position.update(data)
            if text.startswith('debt='):
                for key in ('debt', 'collateral', 'collateral_value'):
                    match = re.search(r'\b' + key + r'=([^ ]+) (BASE|QUOTE)\b', text)
                    if match:
                        self.position[key + '_display'] = amount(match[1]) + ' ' + match[2].lower()
            if text.startswith('debt_mint='):
                return self.health_position()
            return []
        if text.startswith('liquidation_progress '):
            stage = text.split('stage=', 1)[-1]
            message = {'searching ordinary liquidation routes': 'Checking repayment from wallet inventory.',
                       'searching emergency liquidation sizes': 'Checking emergency liquidation amounts.'}.get(stage, readable(stage))
            return self.once((data.get('position'), 'progress', stage),
                             'Position ' + short(data.get('position', '?')) + ': ' + message)
        if text.startswith('liquidation_detail '):
            return self.liquidation(data)
        if text.startswith('position_decision '):
            signature = data.get('signature', 'none')
            if signature != 'none':
                return self.submitted(signature)
            return self.once((data.get('position'), data.get('debt_side'), 'decision'),
                             'Position ' + short(data.get('position', '?')) + ': ' + reason(data.get('detail')), '33')
        if text.startswith('keeper executed '):
            return self.submitted(data.get('signature', '-'))
        if text.startswith(('keeper unclassified ', 'keeper submit_failed ')):
            detail = text.split('detail=', 1)[-1]
            prefix = 'Submission needs attention' if 'submit_failed' in text else 'Could not execute'
            return self.once((data.get('position'), prefix), prefix + ' for ' +
                             short(data.get('position', '?')) + ': ' + reason(detail), '33')
        if text.startswith('keeper snapshot_retry '):
            return []  # Successful internal retries belong in the diagnostic file.
        if text.startswith('keeper profile='):
            return [('Keeper started in ' + ('live execution' if data.get('mode') == 'live' else 'monitoring') + ' mode.', '36')]
        if text.startswith('execution dependencies verified;'):
            return [('Startup checks passed. No transaction sent by this check.', '32')]
        if text.startswith('fee_notice_final '):
            return self.once('fee-change', 'Token fees changed. Rechecking affected positions.', '33')
        if text.startswith('receipt_details_unavailable '):
            return [('The transaction outcome was saved, but its payment and reward details could not be displayed. See the detailed log.', '33')]
        if 'panicked at' in text:
            return [('The keeper stopped unexpectedly. The detailed error is saved in the log.', '31')]
        if text.startswith('Error:'):
            return [('Error: ' + reason(text), '31')]
        if text.startswith('keeper liquidation ') and 'compute limit' in text:
            return self.once('compute', 'The attempted liquidation is too complex for one transaction. Checking other amounts.', '33')
        # Never fall back to dumping unknown machine output into the terminal.
        return self.once('unrecognized', 'Additional diagnostics were saved to the detailed log.', '2')

    def health_position(self):
        p, self.position = self.position, None
        key = (p.get('position'), p.get('debt_mint'))
        state = p.get('state', 'UNAVAILABLE')
        label = {'HEALTHY': 'Healthy', 'LIQUIDATABLE': 'Ready for liquidation', 'DEBT_FREE': 'No debt'}.get(state, 'Health unavailable')
        factor = amount(p.get('health_factor', 'n/a'))
        message = (f'{label} · Position {short(p.get("position", "?"))} · Health {factor}\n'
                   f'  Debt {p.get("debt_display", "unknown")} · Collateral {p.get("collateral_display", "unknown")}'
                   f' · Collateral value {p.get("collateral_value_display", "unknown")}')
        if p.get('scope') != 'supported market':
            message += '\n  Monitoring only: ' + reason(p.get('scope'))
        try:
            bucket = Decimal(factor).quantize(Decimal('0.01'))
        except InvalidOperation:
            bucket = factor
        # Minor interest changes should not repeat an unchanged healthy position
        # every pass. State/meaningful health changes are shown immediately.
        previous = self.positions.get(key)
        now = self.clock()
        fingerprint = (state, bucket, p.get('scope'))
        if previous and previous[0] == fingerprint and now - previous[1] < 60:
            return []
        self.positions[key] = (fingerprint, now)
        if len(self.positions) > 4096:
            self.positions.pop(next(iter(self.positions)))
        return [(message, '33' if state == 'LIQUIDATABLE' else '32')]

    def submitted(self, signature):
        if signature in self.finalized or signature in ('-', 'none'):
            return []
        return self.once(('submitted', signature), 'Transaction submitted; waiting for final confirmation.\n  Signature: ' + signature, '36', 86400)

    def liquidation(self, data):
        stage, detail = data.get('stage', ''), data.get('detail', '')
        prefix = 'Position ' + short(data.get('position', '?')) + ': '
        values = fields(detail)
        if stage in ('ordinary_preview', 'emergency_preview', 'simulation_accepted', 'inventory_quote'):
            return []  # Candidate search is noisy; show the resulting simulation instead.
        if stage == 'ordinary_inventory':
            message = 'Wallet has ' + amount(values.get('balance', '?')) + ' debt tokens available for repayment.'
        elif stage == 'inventory_simulation':
            message = ('Simulation only — would pay ' + amount(values.get('wallet_payment', '?')) +
                       ' of token ' + short(values.get('debt_mint', '?')) + '; receive ' +
                       amount(values.get('collateral_received', '?')) + ' of token ' +
                       short(values.get('collateral_mint', '?')) + '. No tokens spent yet.')
        elif stage == 'ordinary_selected':
            message = ('Full' if 'full' in detail else 'Partial') + ' liquidation passed its simulation.'
        elif stage == 'pre_submit_revalidation':
            message = 'Rechecking the position immediately before sending.'
        elif 'rejected' in stage:
            message = 'Attempt rejected: ' + reason(detail)
        elif 'unavailable' in stage:
            message = 'No executable liquidation at this step: ' + reason(detail)
        else:
            return []
        return self.once((data.get('position'), stage), prefix + message, '33' if 'rejected' in stage or 'unavailable' in stage else '36')

    def receipt(self, text):
        if text == '=== END RECEIPT ===':
            self.in_receipt = False
            return [('End of transaction receipt.', '2')]
        if text.startswith(('Slot:', 'Compute used:')):
            return []
        if text.startswith('Signature: '):
            signature = text.split(': ', 1)[1]
            self.finalized.add(signature)
            if len(self.finalized) > 4096:
                self.finalized.pop()
            return [('  Transaction: https://solscan.io/tx/' + signature, '36')]
        if text.startswith('Error:'):
            return [('  Failed: ' + reason(text), '31')]
        # These receipts already contain decimal amounts and explicit accounting
        # labels. Retain all payments, rewards, swap legs, fees and transfers.
        text = re.sub(r'\b(position|buyer|recipient|mint|owner)=([^\s|]+)',
                      lambda m: {'mint': 'token', 'buyer': 'payer'}.get(m[1], m[1]) + ' ' + short(m[2]), text)
        text = re.sub(r'^\[[\d.]+\] ', '', text)
        text = text.replace('NET PAID -', 'NET PAID ').replace('NET RECEIVED +', 'NET RECEIVED ')
        text = re.sub(r' \(\d+ lamports; includes priority fee\)', ' (including priority fee)', text)
        text = re.sub(r' \| instruction [\d.]+ \| actual hops in transfers below', '; transfers shown below', text)
        for old, new in [('NET PAID', 'Paid'), ('NET RECEIVED', 'Received'), ('UNCHANGED', 'Unchanged'),
                         ('REWARD RECEIVED', 'Reward received'), ('SWAP EXECUTED', 'Swap completed'),
                         ('SWAP ROUTE', 'Swap route'), ('EMERGENCY LIQUIDATION', 'Emergency liquidation'),
                         ('FLASH LIQUIDATION', 'Ordinary liquidation'), ('KEEPER TOKEN BALANCES', 'Wallet token balances'),
                         ('TRANSFERS / MINTS / BURNS', 'Token movements'), ('TOKEN ACCOUNT CHANGES', 'Token balance changes'),
                         ('SOL ACCOUNT CHANGES', 'SOL balance changes'), ('TRANSFER', 'Transfer'),
                         ('MINT', 'Mint'), ('BURN', 'Burn')]:
            text = text.replace(old, new)
        text = re.sub(r'^IN\s+', 'Swap input: ', text)
        text = re.sub(r'^OUT\s+', 'Swap output: ', text)
        if '{' in text or re.search(r'\b\w+=', text):
            return [('  Receipt detail: ' + reason(text), '33')]
        color = '33' if text.startswith(('Paid', 'SOL fee')) else '32' if text.startswith(('Received', 'Reward received')) else '2'
        return [('  ' + readable(text), color)]

    def health(self, snapshot):
        status = snapshot.get('status')
        previous, self.last_health = self.last_health, status
        if status == 'degraded':
            return self.once('status', 'Needs attention: ' + reason(snapshot.get('reason')), '33')
        if status == 'starting':
            return self.once('status', 'Connecting and checking the first batch of positions.', '36')
        if status == 'ready':
            if previous == 'degraded':
                self.seen.pop('status', None)
                return [('Connection and execution checks recovered. Scanning continues.', '32')]
            return self.once('status', 'Running. Watching for eligible work.', '2')
        return []
