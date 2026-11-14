"""Bounded actual CLI ownership checks; only child processes receive signals."""
import ctypes
import errno
import fcntl
import importlib.util
import json
import os
from pathlib import Path
import select
import signal
import struct
import subprocess
import sys
import tempfile
import termios
import time

spec = importlib.util.spec_from_file_location("prior", Path(__file__).parents[2] / "src/apps/pty_test_support.py")
prior = importlib.util.module_from_spec(spec)
sys.dont_write_bytecode = True
spec.loader.exec_module(prior)

ENCODER = r'''#!/usr/bin/env python3
import os,sys,time,subprocess
from pathlib import Path
root=Path(os.environ['CLI_INTERRUPT_ROOT'])
args=sys.argv[1:]
if '--version' in args:
 print('x264 0.164.3108'); sys.exit(0)
if '--zones' in args and args[args.index('--zones')+1]=='0,0,crf=30':
 sys.stdin.buffer.read(); sys.exit(0)
out=Path(args[args.index('-o')+1])
def identity(pid):
 fields=Path(f'/proc/{pid}/stat').read_text().rsplit(') ',1)[1].split()
 return f'{pid} {fields[19]} {fields[2]}\n'
grandchild=subprocess.Popen(['sleep','300'])
(root/'pids').write_text(identity(os.getpid())+identity(grandchild.pid))
with (root/'invocations').open('a') as f: f.write(str(out)+'\n')
out.write_bytes(b'partial')
(root/'active').touch()
deadline=time.monotonic()+15
while not (root/'release').exists():
 if time.monotonic()>deadline: raise RuntimeError('encoder release watchdog')
 time.sleep(.01)
sys.stdin.buffer.read()
grandchild.terminate(); grandchild.wait()
out.write_bytes(b'complete')
(root/'completed').touch()
'''


def run(binary: str, template: Path, base: Path, mode: str, route: str) -> None:
    """Exercise dispatch through a joined encoder/index owner and real input."""
    with tempfile.TemporaryDirectory(prefix='cli-interrupt-', dir=base) as directory:
        root = Path(directory)
        config = json.loads(template.read_text())
        # All mutable products are case-local; source is an immutable sentinel.
        config['temp'] = str(root / 'work')
        config['condor']['output']['path'] = str(root / 'output.mkv')
        if mode.startswith('open') or mode.startswith('override'):
            data = {'VapourSynth': {
                'path': str(root/'media.mkv'),
                'import_method': {'DGDecNV': {'dgindexnv_executable': str(base/'encoder')}},
                'cache_path': None, 'filters': [],
            }}
            (root/'media.mkv').write_bytes(b'source-index-sentinel')
            if mode.startswith('open'):
                config['condor']['input'] = data
            else:
                config['condor']['sequence_config']['parallel_encoder']['input'] = data
        for name in ('parallel_encoder', 'scene_concatenator'):
            config['condor']['sequence_config'][name]['scenes_directory'] = str(root / 'work/scenes')
        job = root / 'job.json'
        job.write_text(json.dumps(config))
        (root / 'output.mkv').write_bytes(b'prior-output-sentinel')
        # This source-owned pty.py must not shadow the stdlib pty module.
        master, slave = os.openpty()
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack('HHHH', 12, 100, 0, 0))
        original = termios.tcgetattr(slave)
        env = os.environ.copy()
        env.pop('CONDOR_TEST_MODE', None)
        env.update(CLI_INTERRUPT_ROOT=directory, TERM='xterm-256color')
        def controlling_terminal():
            os.setsid()
            fcntl.ioctl(slave, termios.TIOCSCTTY, 0)
        errors = []
        terminal = bytearray()
        with tempfile.TemporaryFile() as stdout, tempfile.TemporaryFile() as stderr:
            child = subprocess.Popen([binary, '--config-file', str(job), '--logs', str(root/'events.log')]
                + (['encode'] if route == 'standalone' else ['--skip-scd']),
                stdin=slave, stdout=stdout, stderr=slave if mode.startswith('key') else stderr,
                env=env, preexec_fn=controlling_terminal)
            try:
                deadline = time.monotonic() + 25
                requests = 0
                observed = 0
                while child.poll() is None:
                    assert time.monotonic() < deadline, (mode, 'CLI watchdog', terminal)
                    if select.select([master], [], [], .01)[0]:
                        terminal.extend(os.read(master, 65536))
                    queries = terminal.count(b'\x1b[6n')
                    while observed < queries:
                        os.write(master, b'\x1b[1;1R'); observed += 1
                    if not (root/'active').exists():
                        continue
                    if mode.startswith('key') and not terminal.count(b'\x1b[?1049h'):
                        continue
                    if requests == 0:
                        if mode.startswith('key'):
                            os.write(master, b'\x03')
                        else:
                            child.send_signal(signal.SIGTERM if mode.startswith('term') else signal.SIGINT)
                        requests = 1
                        sent = time.monotonic()
                    elif requests == 1 and time.monotonic() - sent > .3:
                        assert child.poll() is None, 'first request killed active encoder'
                        assert not (root/'completed').exists()
                        if mode.endswith('graceful'):
                            (root/'release').touch()
                        elif mode == 'key-force':
                            os.write(master, b'\x03\x03\x03')
                        else:
                            child.send_signal(signal.SIGINT if mode.startswith('key') else signal.SIGTERM)
                        requests = 2
                status = child.wait(timeout=5)
                restored = termios.tcgetattr(slave)
                os.close(slave); slave = -1
                while True:
                    try:
                        chunk = os.read(master, 65536)
                    except OSError as error:
                        if error.errno != errno.EIO: raise
                        break
                    if not chunk: break
                    terminal.extend(chunk)
                stdout.seek(0); data = stdout.read()
                stderr.seek(0); diagnostic = bytes(terminal) if mode.startswith('key') else stderr.read()
                assert status == (143 if mode.startswith('term') else 130), (mode, status, data, diagnostic)
                assert requests == 2
                assert restored == original, 'raw terminal mode not restored'
                if mode.startswith('key'):
                    assert terminal.count(b'\x1b[?1049h') == terminal.count(b'\x1b[?1049l') == 1
                    assert terminal.count(b'\x1b[?25h') == 1
                    assert diagnostic.index(b'\x1b[?25h') < diagnostic.index(b'Error:')
                else:
                    assert b'\x1b' not in diagnostic
                assert b'\x1b' not in data and b'cancel' not in data.lower(), data
                assert b'cancel' in diagnostic.lower() or b'interrupt' in diagnostic.lower(), diagnostic
                assert (root/'output.mkv').read_bytes() == b'prior-output-sentinel'
                assert 'CLI job cleanup complete' in (root/'events.log').read_text(), 'log guard did not flush final joined marker'
                assert len((root/'invocations').read_text().splitlines()) == 1, 'next scene started'
                saved = json.loads(job.read_text())['condor']['scenes']
                scenes = root/'work/scenes'
                if mode.endswith('graceful'):
                    assert (root/'completed').exists(), 'active encoder did not finish'
                    assert any(p.read_bytes() == b'complete' for p in scenes.iterdir() if p.is_file())
                    completed = saved[0]['sequence_data']['parallel_encoder']
                    waiting = saved[1]['sequence_data']['parallel_encoder']
                    assert completed['provenance'] and completed['completed_on'] and completed['bytes'] == 8, saved
                    assert waiting['provenance'] is None and waiting['completed_on'] is None and waiting['bytes'] is None, saved
                else:
                    assert not (root/'completed').exists()
                    assert not scenes.exists() or not any(p.is_file() and p.suffix == '.264' for p in scenes.iterdir())
                    assert all(scene['sequence_data']['parallel_encoder']['provenance'] is None for scene in saved), saved
                if mode.startswith(('open', 'override')):
                    assert not (root/'media.dgi').exists(), 'forced index published'
                    assert (root/'media.mkv').read_bytes() == b'source-index-sentinel'
                leader = int((root/'pids').read_text().split()[0])
                assert not Path(f'/proc/{leader}').exists(), 'direct child not reaped at outcome'
                print(f'{mode}/{route}: status={status}, joined/restored, one scene, sentinels, child reaped')
            finally:
                if child.poll() is None: child.kill()
                child.wait(timeout=5)
                if (root/'pids').exists():
                    prior.cleanup_fixtures((root/'pids').read_text().splitlines(), errors)
                if slave >= 0: os.close(slave)
                os.close(master)
                if errors:
                    raise AssertionError(f'{sys.exception()}; cleanup: {errors}')
        assert not errors, errors


def listener(binary: str, base: Path, phase: int) -> None:
    """Drive the selected stage reuse, job re-registration or pending-signal join."""
    tests = {
        1: 'apps::tests::internal_integration::interrupt_listener_across_stages_preserves_first_signal_without_duplicate_delivery',
        2: 'apps::tests::internal_integration::interrupt_listener_after_join_can_register_a_fresh_job',
        3: 'apps::tests::internal_integration::interrupt_listener_join_drains_an_already_received_sigterm',
    }
    test = tests[phase]
    env = os.environ.copy()
    env['INTERRUPT_LISTENER_ROOT'] = str(base)
    env['INTERRUPT_LISTENER_TEST'] = test
    with tempfile.TemporaryFile() as output:
        child = subprocess.Popen([binary, '--exact', test, '--nocapture'],
                                 stdin=subprocess.DEVNULL, stdout=output, stderr=output, env=env)
        try:
            deadline = time.monotonic() + 25
            for invocation in range(1 if phase == 1 else 2):
                for marker, event in ((f'ready-{invocation}', signal.SIGTERM if invocation == 0 else signal.SIGINT),
                                      (f'graceful-{invocation}', signal.SIGINT if invocation == 0 else signal.SIGTERM)):
                    while not (base/marker).exists():
                        assert child.poll() is None and time.monotonic() < deadline, ('listener marker', marker)
                        time.sleep(.005)
                    child.send_signal(event)
            status = child.wait(timeout=10)
            output.seek(0)
            text = output.read()
            assert status == 0, text
            assert b'1 passed; 0 failed' in text, text
            assert f'INTERRUPT_LISTENER_PHASE_{phase}_ASSERTIONS_COMPLETED'.encode() in text, text
            print(text.decode())
            print(f'listener phase {phase}: selected child completed')
        finally:
            if child.poll() is None: child.kill()
            child.wait(timeout=5)


def native_pipeline_session_records(session: int) -> list[str]:
    """Private child sessions include encoder groups even after leader exit."""
    records = []
    for path in Path('/proc').glob('[0-9]*/stat'):
        try:
            fields = path.read_text().rsplit(') ', 1)[1].split()
            if int(fields[3]) == session:
                records.append(f'{path.parent.name} {fields[19]} {fields[2]}')
        except (FileNotFoundError, ProcessLookupError):
            continue
    return records


def native_pipeline_command(args: list[str], root: Path, *, env: dict | None = None) -> subprocess.CompletedProcess:
    """Bound every actual stage/native decoder; retain its diagnostics on failure."""
    child = subprocess.Popen(args, cwd=root, env=env, stdout=subprocess.PIPE,
                             stderr=subprocess.PIPE, start_new_session=True)
    completed = False
    try:
        stdout, stderr = child.communicate(timeout=25)
        completed = True
        return subprocess.CompletedProcess(args, child.returncode, stdout, stderr)
    finally:
        if not completed:
            records = native_pipeline_session_records(child.pid)
            if child.poll() is None:
                child.kill()
            child.wait(timeout=5)
            errors = []
            prior.cleanup_fixtures(records, errors)
            assert not errors, errors


def native_pipeline_decode(path: Path) -> bytes:
    result = native_pipeline_command(['ffmpeg', '-v', 'error', '-i', str(path), '-f',
                          'rawvideo', '-pix_fmt', 'yuv420p', '-'], path.parent)
    assert result.returncode == 0, result.stderr
    return result.stdout


def native_pipeline_pattern(pixels: bytes) -> bool:
    """Literal frame signatures: ±3 is disjoint from the next frame's luma (+8)."""
    levels = [32, 40, 48, 56, 64, 72, 80, 88, 96, 104, 112, 120, 128, 136, 144, 152]
    if len(pixels) != 16 * 24576:
        return False
    for n, level in enumerate(levels):
        frame = pixels[n * 24576:(n + 1) * 24576]
        for plane, width, height, left, right in [
            (frame[:16384], 128, 128, level, level + 16),
            (frame[16384:20480], 64, 64, 64 + 4*n, 72 + 4*n),
            (frame[20480:], 64, 64, 192 - 4*n, 184 - 4*n),
        ]:
            for row in range(height):
                for col in range(width):
                    if abs(plane[row*width + col] - (left if col < width//2 else right)) > 3:
                        return False
    return True


NATIVE_PIPELINE_ENCODER = r'''#!/usr/bin/env python3
import os,select,subprocess,sys
from pathlib import Path
if '--version' in sys.argv:
 os.execvp('x264',['x264',*sys.argv[1:]])
root=Path(os.environ['NATIVE_PIPELINE_ROOT'])
args=sys.argv[1:]
out=Path(args[args.index('-o')+1])
with (root/'invocations').open('a') as log: log.write(str(out)+'\n')
grandchild=subprocess.Popen([sys.executable,'-c','import time; time.sleep(300)'])
def identity(pid):
 fields=Path(f'/proc/{pid}/stat').read_text().rsplit(') ',1)[1].split()
 return f'{pid} {fields[19]} {fields[2]}\n'
(root/'pids.tmp').write_text(identity(os.getpid())+identity(grandchild.pid))
(root/'pids.tmp').replace(root/'pids')
try:
 if 'NATIVE_PIPELINE_RELEASE_ALL' not in os.environ:
  out.write_bytes(b'owned incomplete native attempt')
  gate=os.open(root/'release',os.O_RDONLY|os.O_NONBLOCK)
  try:
   (root/'active').touch()
   ready=select.select([gate],[],[],15)[0]
   command=os.read(gate,1) if ready else b''
   # T injects the watchdog boundary in the test-local counterexample only.
   if not ready or command==b'T':
    (root/'gate-timeout').write_text('native gate watchdog')
    raise AssertionError('native gate watchdog')
   assert command==b'1'
  finally:
   os.close(gate)
 result=subprocess.run(['x264',*args],stdin=sys.stdin.buffer)
finally:
 grandchild.terminate(); grandchild.wait(timeout=5)
sys.exit(result.returncode)
'''


def native_pipeline_pty(binary: str, base: Path, template: dict, forced: bool,
            *, gate_timeout: bool = False) -> None:
    """Controlling-terminal protocol, now with real completed native chunks."""
    with tempfile.TemporaryDirectory(prefix='native-pipeline-pty-', dir=base) as directory:
        root = Path(directory)
        config = json.loads(json.dumps(template))
        config['temp'] = str(root/'work')
        config['condor']['output']['path'] = str(root/'final.mkv')
        sequence = config['condor']['sequence_config']
        sequence['target_quality'] = None
        sequence['quality_check'] = None
        for name in ('parallel_encoder', 'scene_concatenator'):
            sequence[name]['scenes_directory'] = str(root/'work/scenes')
        wrapper = base/'native-encoder'
        for encoder in [config['condor']['encoder'], *[s['encoder'] for s in config['condor']['scenes']]]:
            encoder['X264']['executable'] = str(wrapper)
        job = root/'job.json'
        job.write_text(json.dumps(config))
        sentinel = root/'final.mkv'
        sentinel.write_bytes(b'preserve prior output')
        source = (base/'source.vpy').read_bytes()
        os.mkfifo(root/'release')
        master, slave = os.openpty()
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack('HHHH', 16, 110, 0, 0))
        original = termios.tcgetattr(slave)
        env = os.environ.copy()
        env.pop('CONDOR_TEST_MODE', None)
        env.update(NATIVE_PIPELINE_ROOT=directory, TERM='xterm-256color')
        terminal = bytearray()
        errors = []
        records = []
        def controlling_terminal():
            os.setsid()
            fcntl.ioctl(slave, termios.TIOCSCTTY, 0)
        with tempfile.TemporaryFile() as stdout:
            child = subprocess.Popen([binary, '--config-file', str(job), '--logs', str(root/'events.log'), 'encode'],
                cwd=root, stdin=slave, stdout=stdout, stderr=slave, env=env,
                preexec_fn=controlling_terminal)
            try:
                deadline = time.monotonic() + 25
                queries = 0
                sent = False
                acknowledged = False
                while child.poll() is None:
                    assert time.monotonic() < deadline, ('native pipeline PTY watchdog', terminal)
                    if select.select([master], [], [], .01)[0]:
                        terminal.extend(os.read(master, 65536))
                    while queries < terminal.count(b'\x1b[6n'):
                        os.write(master, b'\x1b[1;1R'); queries += 1
                    if not sent and (root/'active').exists() and b'\x1b[?1049h' in terminal:
                        raw = termios.tcgetattr(slave)
                        assert not raw[3] & (termios.ICANON | termios.ECHO), 'native raw mode not entered'
                        records = (root/'pids').read_text().splitlines()
                        assert len(records) == 2
                        for record in records:
                            pid, start, group = map(int, record.split())
                            fd = prior.fixture_pidfd(pid, (start, group))
                            try:
                                assert not select.select([fd], [], [], 0)[0], 'child tree must actually be live'
                            finally:
                                os.close(fd)
                        os.write(master, b'\x03')
                        sent = True
                    if sent and not acknowledged and b'after Encoders complete...' in terminal:
                        assert len((root/'invocations').read_text().splitlines()) == 1
                        assert child.poll() is None
                        if forced and not gate_timeout:
                            os.write(master, b'\x03')
                        else:
                            try:
                                gate = os.open(root/'release', os.O_WRONLY | os.O_NONBLOCK)
                            except OSError as error:
                                if error.errno != errno.ENXIO:
                                    raise
                                continue
                            os.write(gate, b'T' if gate_timeout else b'1'); os.close(gate)
                        acknowledged = True
                status = child.wait(timeout=5)
                assert status == 130 and sent and acknowledged, (status, terminal)
                assert termios.tcgetattr(slave) == original, 'native terminal not restored'
                os.close(slave); slave = -1
                while select.select([master], [], [], 0)[0]:
                    try:
                        chunk = os.read(master, 65536)
                        if not chunk:
                            break
                        terminal.extend(chunk)
                    except OSError as error:
                        if error.errno != errno.EIO:
                            raise
                        break
                assert terminal.count(b'\x1b[?1049h') == terminal.count(b'\x1b[?1049l') == 1
                assert terminal.count(b'\x1b[?25h') == 1
                stdout.seek(0)
                assert b'\x1b' not in stdout.read()
                assert 'CLI job cleanup complete' in (root/'events.log').read_text()
                assert len((root/'invocations').read_text().splitlines()) == 1, 'new work admitted after first interrupt'
                saved = json.loads(job.read_text())['condor']['scenes']
                chunks = sorted((root/'work/scenes').glob('*.264'))
                if forced:
                    assert not list((root/'work/scenes').rglob('*.264')), 'forced partial native output survived'
                    assert all(s['sequence_data']['parallel_encoder']['provenance'] is None for s in saved)
                else:
                    assert len(chunks) == 1
                    assert len(native_pipeline_decode(chunks[0])) == 4 * 24576, 'saved partial scene must be genuinely decodable'
                    assert saved[0]['sequence_data']['parallel_encoder']['provenance']
                    assert saved[0]['sequence_data']['parallel_encoder']['completed_on']
                    assert saved[0]['sequence_data']['parallel_encoder']['bytes'] == chunks[0].stat().st_size
                    assert all(s['sequence_data']['parallel_encoder']['bytes'] is None for s in saved[1:])
                for record in records:
                    pid, start, group = map(int, record.split())
                    if Path(f'/proc/{pid}').exists():
                        fd = prior.fixture_pidfd(pid, (start, group))
                        try:
                            assert select.select([fd], [], [], 0)[0], f'owned PID {pid} still alive at CLI outcome'
                        finally:
                            os.close(fd)
                assert not (root/'gate-timeout').exists(), 'native pipeline gate watchdog fired before CLI outcome'
                prior.cleanup_fixtures(records, errors)
                assert not errors, errors
                assert all(not Path(f'/proc/{record.split()[0]}').exists() for record in records), 'owned tree not reaped'
                assert sentinel.read_bytes() == b'preserve prior output'
                assert (base/'source.vpy').read_bytes() == source
                print(f'native pipeline PTY forced={forced}: actual raw/alternate enter+restore, acknowledged first interrupt, status130, next admission stopped, no gate timeout before cleanup, owned wrapper/grandchild exited+reaped; pids={records}', flush=True)
                if not forced:
                    first = chunks[0]
                    frozen = (first.read_bytes(), first.stat().st_mtime_ns)
                    env['CONDOR_TEST_MODE'] = '1'
                    env['NATIVE_PIPELINE_RELEASE_ALL'] = '1'
                    partial = job.read_bytes()
                    stale_partial = json.loads(partial)
                    stale_partial['condor']['scenes'][0]['encoder']['X264']['options']['ref'] = {
                        'Number': {'prefix': '--', 'delimiter': ' ', 'value': 3.0}}
                    job.write_text(json.dumps(stale_partial))
                    before = job.read_bytes()
                    result = native_pipeline_command([binary, '--config-file', str(job), 'encode'], root, env=env)
                    assert result.returncode != 0 and b'context changed' in result.stderr, result.stderr
                    assert job.read_bytes() == before
                    assert len((root/'invocations').read_text().splitlines()) == 1
                    assert (first.read_bytes(), first.stat().st_mtime_ns) == frozen
                    assert len(list((root/'work/scenes').glob('*.264'))) == 1
                    # Restore the actual producer's partial JSON, never a new stamp.
                    job.write_bytes(partial)
                    result = native_pipeline_command([binary, '--config-file', str(job), 'encode'], root, env=env)
                    assert result.returncode == 0, (result.stdout, result.stderr)
                    assert (first.read_bytes(), first.stat().st_mtime_ns) == frozen
                    assert len((root/'invocations').read_text().splitlines()) == 3, 'restart encoded completed scene or missed unfinished work'
                    completed = sorted((root/'work/scenes').glob('*.264'))
                    decoded = [native_pipeline_decode(p) for p in completed]
                    assert [len(p) for p in decoded] == [98304, 196608, 98304]
                    assert native_pipeline_pattern(b''.join(decoded))
                    all_frozen = [(p, p.read_bytes(), p.stat().st_mtime_ns) for p in completed]
                    result = native_pipeline_command([binary, '--config-file', str(job), 'encode'], root, env=env)
                    assert result.returncode == 0 and len((root/'invocations').read_text().splitlines()) == 3
                    stale = json.loads(job.read_text())
                    stale['condor']['scenes'][0]['encoder']['X264']['options']['ref'] = {
                        'Number': {'prefix': '--', 'delimiter': ' ', 'value': 3.0}}
                    job.write_text(json.dumps(stale))
                    before = job.read_bytes()
                    result = native_pipeline_command([binary, '--config-file', str(job), 'encode'], root, env=env)
                    assert result.returncode != 0 and b'context changed' in result.stderr, result.stderr
                    assert job.read_bytes() == before
                    assert len((root/'invocations').read_text().splitlines()) == 3
                    assert all((p.read_bytes(), p.stat().st_mtime_ns) == (b, m) for p, b, m in all_frozen)
                    assert sentinel.read_bytes() == b'preserve prior output'
                    assert (base/'source.vpy').read_bytes() == source
                    print('native pipeline restart: stale partial identity refused before native work; real partial scene reused bytes+mtime; exactly two unfinished native encodes; 16 ordered frames; matching no-work restart and stale completed identity refusal preserve all chunks/source/sentinel PASS', flush=True)
            finally:
                emergency = native_pipeline_session_records(child.pid) if child.poll() is None else []
                if child.poll() is None:
                    child.kill()
                child.wait(timeout=5)
                if (root/'pids').exists():
                    prior.cleanup_fixtures((root/'pids').read_text().splitlines(), errors)
                prior.cleanup_fixtures(emergency, errors)
                if slave >= 0:
                    os.close(slave)
                os.close(master)
                assert not errors, errors


def native_pipeline(binary: str, root: Path) -> None:
    """Actual CLI partition/scoring/encoding and an independent order oracle."""
    assert ctypes.CDLL(None, use_errno=True).prctl(36, 1, 0, 0, 0) == 0
    source = (root/'source.vpy').read_bytes()
    job = root/'condor.json'
    env = os.environ.copy()
    env['CONDOR_TEST_MODE'] = '1'
    def stage(*args: str, success: bool = True) -> subprocess.CompletedProcess:
        result = native_pipeline_command([binary, '--config-file', str(job), *args], root, env=env)
        print(f'native pipeline stage {args}: exit={result.returncode}', flush=True)
        assert (result.returncode == 0) == success, (args, result.stdout, result.stderr)
        assert (root/'source.vpy').read_bytes() == source
        return result
    stage('detect-scenes')
    detected = json.loads(job.read_text())
    assert [(s['start_frame'], s['end_frame']) for s in detected['condor']['scenes']] == [(0, 16)]
    stage('zone', '4 12 x264 "--ref 2"')
    def association(data: dict) -> None:
        scenes = data['condor']['scenes']
        assert [(s['start_frame'], s['end_frame']) for s in scenes] == [(0, 4), (4, 12), (12, 16)]
        assert [s['encoder']['X264']['options'].get('ref') for s in scenes] == [
            None, {'Number': {'prefix': '--', 'delimiter': ' ', 'value': 2.0}}, None]
    zoned = json.loads(job.read_text())
    association(zoned)
    assert all(s['encoder']['X264']['options']['crf']['Number']['value'] == 20
               for s in zoned['condor']['scenes']), 'TQ fixture must start outside 0..12'
    assert zoned['condor']['scenes'][0]['sequence_data']['zoner']['lifecycle']
    for args in [('target-quality',), ('encode',), ('concatenate',), ('quality-check',)]:
        stage(*args)
        association(json.loads(job.read_text()))
    measured = json.loads(job.read_text())
    for scene, frames in zip(measured['condor']['scenes'], (4, 8, 4)):
        data = scene['sequence_data']
        assert data['target_quality']['selection']
        selected_q = scene['encoder']['X264']['options']['crf']['Number']['value']
        assert 0 <= selected_q <= 12 and selected_q != 20, 'TQ must replace the initial CRF20'
        assert any(p['quantizer'] == selected_q for p in data['target_quality']['passes'])
        print(f'native pipeline TQ propagation: initial=20 saved={selected_q} measured={[p["quantizer"] for p in data["target_quality"]["passes"]]}', flush=True)
        for record in [*data['target_quality']['passes'], data['quality_check']['quality']]:
            assert record['provenance'] and record['backend']['engine'] == 'Vmaf'
            assert len(record['scores']) == frames
            assert 98 <= sum(record['scores']) / frames <= 100
            print(f"native pipeline range={scene['start_frame']}..{scene['end_frame']} q={record['quantizer']} scores={record['scores']}")
        assert data['parallel_encoder']['provenance'] and data['parallel_encoder']['bytes'] > 0
    decoded = native_pipeline_decode(root/'final.mkv')
    assert native_pipeline_pattern(decoded), 'final frames violate literal pattern/order/±3 codec bound'
    assert not native_pipeline_pattern(decoded[24576:49152] + decoded[:24576] + decoded[49152:]), 'order oracle negative control'
    chunks = sorted((root/'work/scenes').glob('*.264'))
    assert len(chunks) == 3
    assert [len(native_pipeline_decode(p)) for p in chunks] == [98304, 196608, 98304]
    frozen = [(path, path.read_bytes(), path.stat().st_mtime_ns) for path in chunks]
    stage('target-quality')
    stage('encode')
    assert json.loads(job.read_text())['condor']['scenes'] == measured['condor']['scenes']
    assert all((p.read_bytes(), p.stat().st_mtime_ns) == (b, m) for p, b, m in frozen)
    changed = json.loads(job.read_text())
    changed['condor']['scenes'][0]['encoder']['X264']['options']['ref'] = {
        'Number': {'prefix': '--', 'delimiter': ' ', 'value': 3.0}}
    job.write_text(json.dumps(changed))
    before = job.read_bytes()
    refused = stage('encode', success=False)
    assert b'context changed' in refused.stderr and b'cannot resume' in refused.stderr, refused.stderr
    assert job.read_bytes() == before
    assert all((p.read_bytes(), p.stat().st_mtime_ns) == (b, m) for p, b, m in frozen)
    job.write_text(json.dumps(measured))
    print('native pipeline: actual detect/zone/TQ/encode/concat/QC, 16 ordered patterned frames, parameters/provenance/source unchanged; current reuse/stale refusal PASS', flush=True)
    wrapper = root/'native-encoder'
    wrapper.write_text(NATIVE_PIPELINE_ENCODER)
    wrapper.chmod(0o755)
    # Start fresh PTY jobs with actual TQ-produced encoders, not CRF20 or
    # copied completion/provenance records from the finished pipeline.
    for scene, produced in zip(zoned['condor']['scenes'], measured['condor']['scenes']):
        scene['encoder'] = produced['encoder']
    for forced in (False, True):
        native_pipeline_pty(binary, root, zoned, forced)
    try:
        # Do not send the second interrupt: reproduce the old false-positive
        # outcome by injecting the wrapper's own watchdog failure instead.
        native_pipeline_pty(binary, root, zoned, True, gate_timeout=True)
    except AssertionError as error:
        assert str(error) == 'native pipeline gate watchdog fired before CLI outcome', error
        print('native pipeline watchdog counterexample: ignored second interrupt + test-local gate timeout rejected before test cleanup; owned tree reaped', flush=True)
    else:
        raise AssertionError('native pipeline must reject the ignored-second/gate-timeout counterexample')


if __name__ == '__main__' and sys.argv[1] == '--native-pipeline':
    native_pipeline(sys.argv[2], Path(sys.argv[3]))
elif __name__ == '__main__' and len(sys.argv) == 5 and sys.argv[3] == 'listener':
    listener(sys.argv[1], Path(sys.argv[2]), int(sys.argv[4]))
elif __name__ == '__main__':
    assert ctypes.CDLL(None, use_errno=True).prctl(36, 1, 0, 0, 0) == 0
    binary, base = sys.argv[1:]
    base = Path(base)
    source = (base/'source.vpy').read_bytes()
    encoder = base/'encoder'
    encoder.write_text(ENCODER); encoder.chmod(0o755)
    failures = []
    for route in ('standalone', 'root'):
        for mode in ('key-graceful', 'key-force', 'key-mixed', 'int-graceful', 'int-force', 'term-graceful', 'term-force', 'open-force', 'override-force'):
            try:
                run(binary, base/'template.json', base, mode, route)
            except (AssertionError, OSError, subprocess.TimeoutExpired) as error:
                failures.append(f'{mode}/{route}: {error}')
    assert (base/'source.vpy').read_bytes() == source
    assert not failures, '\n'.join(failures)
