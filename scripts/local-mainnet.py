#!/usr/bin/env python3
"""Local mainnet services with private key files and an isolated PostgreSQL journal.

setup only writes missing private templates. check never signs or submits.
terminal starts LIVE workers; Ctrl+C stops workers before stopping the journal.
"""
import argparse
import datetime
import fcntl
import hashlib
import json
import os
from pathlib import Path
import re
import signal
import socket
import subprocess
import sys
import threading

REPO = Path(__file__).resolve().parent.parent
ROOT = Path(os.environ.get('KEEPER_MAINNET_HOME', str(Path.home() / '.local/share/dusk-keepers/mainnet'))).resolve()
PG = Path(os.environ.get('KEEPER_POSTGRES_BIN', '/opt/homebrew/opt/postgresql@17/bin'))
BINARY = REPO / 'target/debug/dusk-keeper'
PROFILES = ['lending', 'leverage', 'hlp-recovery', 'auction-arbitrage', 'lifecycle', 'sentinel']
PORTS = dict(zip(PROFILES, range(18091, 18097)))
PGPORT = '15440'
DB = 'dusk_keepers_mainnet'
SOCKET = ROOT / 'pgsocket'
PGDATA = ROOT / 'postgres'
ANSI = re.compile(r'\x1b\[[0-9;]*m')
COLORS = dict(zip(PROFILES, ['36', '35', '34', '34', '33', '32']))
OPTIONS = {
    'DISCOVERY_INTERVAL_MS', 'KEEPER_MAX_SCAN_AGE_MS', 'KEEPER_MAX_SENDS_PER_PASS',
    'KEEPER_MINIMUM_LAMPORTS', 'KEEPER_EMERGENCY_SLIPPAGE_BPS', 'KEEPER_ORDER_MIN_BOUNTY_CREDIT',
    'KEEPER_AUCTION_MAX_SOLD', 'KEEPER_AUCTION_MAX_PAYMENT', 'KEEPER_AUCTION_SLIPPAGE_BPS',
    'KEEPER_HLP_MARKET', 'KEEPER_HLP_MAX_SOCIALIZED_LOSS', 'KEEPER_HLP_MAX_PAYMENT_DEBIT',
    'KEEPER_HLP_MIN_COLLATERAL_CREDIT', 'KEEPER_HLP_MIN_BOUNTY_OUT',
}


def private_file(path, content):
    """Never replace a wallet, journal, RPC credential or existing operator policy."""
    if not path.exists():
        with path.open('x') as handle:
            handle.write(content)
    path.chmod(0o600)


def setup():
    for path in [ROOT, ROOT / 'wallets', ROOT / 'logs', SOCKET]:
        path.mkdir(parents=True, exist_ok=True, mode=0o700)
        path.chmod(0o700)
    private_file(ROOT / 'rpc-url', '')
    private_file(ROOT / 'risk-policy.json', (REPO / 'deploy/mainnet/risk-policy.example.json').read_text())
    settings = {'profiles': {}}
    for profile in PROFILES:
        settings['profiles'][profile] = {'environment': {}}
        if profile != 'sentinel':
            key = ROOT / 'wallets' / (profile + '.key')
            private_file(key, '')
            settings['profiles'][profile]['keyPath'] = str(key)
    private_file(ROOT / 'settings.json', json.dumps(settings, indent=2) + '\n')


def pg_env():
    return {'PATH': os.environ.get('PATH', '/usr/bin:/bin'), 'LANG': 'C', 'LC_ALL': 'C', 'LC_CTYPE': 'C'}


def pg_running():
    return subprocess.run([str(PG / 'pg_ctl'), '-D', str(PGDATA), 'status'],
                          env=pg_env(), stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL).returncode == 0


def pg_run(args):
    with (ROOT / 'logs/postgres-setup.log').open('a') as log:
        subprocess.run([str(a) for a in args], env=pg_env(), stdout=log, stderr=log, check=True)


def sql(query, database=DB):
    result = subprocess.run([str(PG / 'psql'), '-X', '-h', str(SOCKET), '-p', PGPORT,
                             '-U', 'dusk_keeper_admin', '-d', database, '-v', 'ON_ERROR_STOP=1', '-Atq'],
                            input=query, text=True, capture_output=True, env=pg_env())
    if result.returncode:
        with (ROOT / 'logs/postgres-setup.log').open('a') as log:
            log.write(result.stderr)
        raise RuntimeError('Database setup failed; see ' + str(ROOT / 'logs/postgres-setup.log'))
    return result.stdout.strip()


def quote(value):
    return "'" + value.replace("'", "''") + "'"


def start_database():
    if not (PGDATA / 'PG_VERSION').exists():
        pg_run([PG / 'initdb', '-D', PGDATA, '-U', 'dusk_keeper_admin', '--auth-local=trust',
                '--auth-host=reject', '--no-locale', '-E', 'UTF8'])
        with (PGDATA / 'postgresql.conf').open('a') as handle:
            handle.write("\nlisten_addresses = ''\nport = " + PGPORT +
                         "\nunix_socket_permissions = 0700\nunix_socket_directories = " + quote(str(SOCKET)) + '\n')
    if not pg_running():
        pg_run([PG / 'pg_ctl', '-D', PGDATA, '-l', ROOT / 'logs/postgres.log', '-w', 'start'])
    if not sql("SELECT 1 FROM pg_roles WHERE rolname='dusk_keeper_runtime';", 'postgres'):
        sql('CREATE ROLE dusk_keeper_runtime LOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE;', 'postgres')
    if not sql('SELECT 1 FROM pg_database WHERE datname=' + quote(DB) + ';', 'postgres'):
        sql('CREATE DATABASE ' + DB + ';', 'postgres')
    sql('CREATE TABLE IF NOT EXISTS keeper_schema_migrations (name text PRIMARY KEY, sha256 text NOT NULL);')
    for path in sorted((REPO / 'deploy/postgres/migrations').glob('*.sql')):
        digest = hashlib.sha256(path.read_bytes()).hexdigest()
        applied = sql('SELECT sha256 FROM keeper_schema_migrations WHERE name=' + quote(path.name) + ';')
        if applied:
            if applied != digest:
                raise RuntimeError('Applied migration has changed: ' + path.name)
            continue
        body = path.read_text().strip()
        if not body.startswith('BEGIN;') or not body.endswith('COMMIT;'):
            raise RuntimeError('Expected a transactional migration: ' + path.name)
        sql('BEGIN;\n' + body[len('BEGIN;'):-len('COMMIT;')] +
            '\nINSERT INTO keeper_schema_migrations VALUES (' + quote(path.name) + ',' + quote(digest) + ');\nCOMMIT;')
    sql('REVOKE CREATE ON SCHEMA public FROM PUBLIC; GRANT CONNECT ON DATABASE ' + DB +
        ' TO dusk_keeper_runtime; GRANT USAGE ON SCHEMA public TO dusk_keeper_runtime; '
        'GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA public TO dusk_keeper_runtime; '
        'REVOKE ALL ON keeper_schema_migrations FROM dusk_keeper_runtime; '
        'GRANT USAGE, SELECT ON ALL SEQUENCES IN SCHEMA public TO dusk_keeper_runtime;')


def environment(profile):
    if not BINARY.exists():
        raise RuntimeError('Build the keeper first: cargo build --locked --bin dusk-keeper')
    rpc = (ROOT / 'rpc-url').read_text().strip()
    if not rpc:
        raise RuntimeError('Add your mainnet RPC URL to ' + str(ROOT / 'rpc-url'))
    settings = json.loads((ROOT / 'settings.json').read_text())['profiles'][profile]
    env = {
        'PATH': os.environ.get('PATH', '/usr/bin:/bin'),
        'KEEPER_MODE': 'live', 'KEEPER_PROFILE': profile,
        'SOLANA_RPC_HTTP_URL': rpc, 'PORT': str(PORTS[profile]),
        'DATABASE_URL': "host=" + quote(str(SOCKET)) + ' port=' + PGPORT + ' user=dusk_keeper_runtime dbname=' + DB,
        'KEEPER_RISK_POLICY': str(ROOT / 'risk-policy.json'),
        'DISCOVERY_INTERVAL_MS': '5000', 'KEEPER_MAX_SCAN_AGE_MS': '120000',
        'KEEPER_MAX_SENDS_PER_PASS': '1', 'KEEPER_MINIMUM_LAMPORTS': '50000000',
        'KEEPER_LOG_POSITIONS': '1', 'KEEPER_LOG_COLOR': 'always' if sys.stdout.isatty() else 'never',
        'KEEPER_LENDING_MODE': 'both', 'KEEPER_FLASH_SETTLEMENT': 'inventory',
        'KEEPER_FLASH_MAX_REPAY': '18446744073709551615',
        'KEEPER_FLASH_MAX_PAYMENT_DEBIT': '18446744073709551615',
        'KEEPER_FLASH_MIN_COLLATERAL_CREDIT': '1', 'KEEPER_LIQUIDATION_MIN_PROFIT': '0',
        'KEEPER_EMERGENCY_SLIPPAGE_BPS': '100', 'KEEPER_ORDER_MIN_BOUNTY_CREDIT': '0',
    }
    if 'NO_COLOR' in os.environ:
        env['NO_COLOR'] = os.environ['NO_COLOR']
        env['KEEPER_LOG_COLOR'] = 'never'
    overrides = settings.get('environment', {})
    for key, value in overrides.items():
        if key not in OPTIONS:
            raise RuntimeError('Unsupported profile setting: ' + key)
        env[key] = str(value)
    for key in OPTIONS:
        if key in os.environ:
            env[key] = os.environ[key]
    if profile != 'sentinel':
        key = Path(settings['keyPath']).expanduser()
        if not key.is_file() or key.stat().st_size == 0:
            raise RuntimeError('Add the ' + profile + ' private key to ' + str(key) + ' (base58 or 64-byte Solana JSON)')
        if key.stat().st_mode & 0o077:
            raise RuntimeError('Restrict the wallet file permissions: chmod 600 ' + str(key))
        env['SIGNER_PROVIDER'] = 'local'
        env['KEEPER_SIGNER_KEY_PATH'] = str(key)
    return env


def identities(environments):
    seen = {}
    for profile, env in environments.items():
        if profile == 'sentinel':
            continue
        address = subprocess.check_output([str(BINARY), '--signer-public-key'], env=env, cwd=REPO, text=True).strip()
        if address in seen:
            raise RuntimeError(profile + ' and ' + seen[address] + ' must use distinct wallets')
        seen[address] = profile
        print(profile + ' wallet: ' + address, flush=True)


def terminal(environments):
    children, readers = [], []
    stopping, output_lock = threading.Event(), threading.Lock()

    def forward(profile, child):
        with (ROOT / 'logs' / (profile + '.log')).open('a', buffering=1) as log:
            for line in child.stdout:
                stamp = datetime.datetime.now().strftime('%H:%M:%S')
                plain = stamp + ' [' + profile + '] ' + ANSI.sub('', line)
                log.write(plain)
                with output_lock:
                    if environments[profile]['KEEPER_LOG_COLOR'] == 'always':
                        print(stamp + ' \x1b[' + COLORS[profile] + 'm[' + profile + ']\x1b[0m ' + line, end='', flush=True)
                    else:
                        print(plain, end='', flush=True)

    previous = {sig: signal.signal(sig, lambda *_: stopping.set()) for sig in (signal.SIGINT, signal.SIGTERM)}
    try:
        print('LIVE MAINNET: ' + ', '.join(environments) + '. Ctrl+C stops this session.', flush=True)
        print('Inventory liquidation pays debt and keeps collateral across all supported markets. Token payments are uncapped.', flush=True)
        for profile, env in environments.items():
            if stopping.is_set():
                break
            child = subprocess.Popen([str(BINARY)], cwd=REPO, env=env, stdout=subprocess.PIPE,
                                     stderr=subprocess.STDOUT, text=True, bufsize=1, start_new_session=True)
            children.append((profile, child))
            reader = threading.Thread(target=forward, args=(profile, child), daemon=True)
            readers.append(reader)
            reader.start()
        while not stopping.wait(1):
            for profile, child in children:
                if child.poll() is not None:
                    raise RuntimeError(profile + ' exited; stopping this session. See ' + str(ROOT / 'logs' / (profile + '.log')))
    finally:
        for _, child in reversed(children):
            if child.poll() is None:
                child.terminate()
                try:
                    child.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    child.kill()
                    child.wait()
        for reader in readers:
            reader.join(timeout=2)
        for sig, handler in previous.items():
            signal.signal(sig, handler)
        print('Stopped. Wallets and transaction journal are preserved.', flush=True)


def main():
    os.umask(0o077)
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('action', choices=['setup', 'check', 'terminal'])
    parser.add_argument('profiles', nargs='*', help='default: leverage; separate key per signing profile')
    args = parser.parse_args()
    profiles = args.profiles or ['leverage']
    if len(set(profiles)) != len(profiles) or any(p not in PROFILES for p in profiles):
        parser.error('Choose distinct profiles from: ' + ', '.join(PROFILES))
    setup()
    if args.action == 'setup':
        print('Private mainnet settings prepared in ' + str(ROOT) + '; no workers started.')
        return
    with (ROOT / 'session.lock').open('a') as lock:
        try:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            raise RuntimeError('Another local mainnet session is already running')
        environments = {profile: environment(profile) for profile in profiles}
        identities(environments)
        if args.action == 'terminal':
            for profile in profiles:
                with socket.socket() as probe:
                    try:
                        probe.bind(('127.0.0.1', PORTS[profile]))
                    except OSError as error:
                        raise RuntimeError('Health port already in use for ' + profile + ': ' + str(PORTS[profile])) from error
        started_database = not pg_running()
        try:
            start_database()
            # This preflight cannot sign or submit; all profiles must pass before any worker starts.
            for env in environments.values():
                subprocess.run([str(BINARY), '--check-execution'], cwd=REPO, env=env, check=True)
            if args.action == 'terminal':
                terminal(environments)
        finally:
            if started_database and pg_running():
                pg_run([PG / 'pg_ctl', '-D', PGDATA, '-m', 'fast', '-w', 'stop'])


if __name__ == '__main__':
    try:
        main()
    except KeyboardInterrupt:
        sys.exit(130)
    except (RuntimeError, OSError, ValueError, KeyError, subprocess.CalledProcessError) as error:
        print('Error: ' + str(error), file=sys.stderr)
        sys.exit(1)
