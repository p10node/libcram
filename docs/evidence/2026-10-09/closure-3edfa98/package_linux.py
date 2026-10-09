from pathlib import Path
import os
import subprocess

revision = '3edfa98d95e039cdaca20e9906de3d10db301f2a'
repo = Path('/root/libcram-sdk-' + revision[:12])
assert not repo.exists(), 'preserve existing checkout'
subprocess.run(['git', 'clone', '/root/libcram/target/final-source.bundle', str(repo)], check=True)
assert subprocess.check_output(['git', '-C', str(repo), 'rev-parse', 'HEAD'], text=True).strip() == revision
cargo_home = Path('/root/libcram/target/sdk-cargo-' + revision[:12])
cargo_home.mkdir(exist_ok=False)
(cargo_home / 'config.toml').write_text(Path('/root/libcram/.cargo/config.toml').read_text())
env = os.environ.copy()
env['CARGO_HOME'] = str(cargo_home)
env['CARGO_TARGET_DIR'] = '/root/libcram/target/sdk-build-' + revision[:12]
subprocess.run(['python3', 'scripts/package_sdk.py'], cwd=repo, env=env, check=True)
archive = repo / 'target/dist' / ('libcram-0.1.0-x86_64-unknown-linux-gnu-' + revision[:12] + '.tar.gz')
subprocess.run(['python3', 'scripts/verify_sdk.py', str(archive)], cwd=repo, env=env, check=True)
assert not subprocess.check_output(['git', '-C', str(repo), 'status', '--porcelain'], text=True)
print('LINUX_SDK_VERIFIED', archive)
