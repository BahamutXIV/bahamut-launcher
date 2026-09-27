import { execFileSync, spawnSync } from 'node:child_process';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const repositoryRoot = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const requiredVersion = '22.1.8';
const args = process.argv.slice(2);

function gitPaths(...arguments_) {
    return execFileSync('git', arguments_, {
        cwd: repositoryRoot,
        encoding: 'utf8',
        stdio: ['ignore', 'pipe', 'pipe'],
    }).split('\0').filter(Boolean);
}

try {
    let paths;
    if (args.length === 0) {
        paths = [
            ...gitPaths('diff', '--name-only', '--diff-filter=ACMR', '-z', 'HEAD', '--'),
            ...gitPaths('ls-files', '--others', '--exclude-standard', '-z'),
        ];
    } else if (args.length === 1 && args[0] === '--all') {
        paths = gitPaths('ls-files', '-z');
    } else if (args.length === 2 && args[0] === '--base' && args[1]) {
        paths = gitPaths('diff', '--name-only', '--diff-filter=ACMR', '-z', `${args[1]}...HEAD`, '--');
    } else {
        throw new Error('Usage: node scripts/check-cpp-format.mjs [--base <ref> | --all]');
    }

    const files = [...new Set(paths)].filter(path => /^client\/(api|src|tests)\/.+\.(cpp|h|hpp)$/.test(path)).sort();
    if (files.length === 0) {
        console.log('C++ format check: no first-party C++ files selected.');
        process.exit(0);
    }

    const version = execFileSync('clang-format', ['--version'], { encoding: 'utf8' }).trim();
    if (version.match(/\bversion\s+(\d+\.\d+\.\d+)\b/)?.[1] !== requiredVersion) {
        throw new Error(`clang-format ${requiredVersion} is required; found ${version}`);
    }

    console.log(`Checking ${files.length} first-party C++ files with clang-format ${requiredVersion}.`);
    const result = spawnSync('clang-format', ['--dry-run', '--Werror', '--style=file', '--', ...files], {
        cwd: repositoryRoot,
        stdio: 'inherit',
    });
    if (result.error) throw result.error;
    process.exit(result.status ?? 1);
} catch (error) {
    console.error(`C++ format check failed: ${error.message}`);
    process.exit(1);
}
