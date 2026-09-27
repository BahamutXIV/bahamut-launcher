import { execFileSync } from 'node:child_process';
import { existsSync, readFileSync } from 'node:fs';
import { dirname, isAbsolute, relative, resolve, sep } from 'node:path';
import { fileURLToPath } from 'node:url';

const repositoryRoot = resolve(dirname(fileURLToPath(import.meta.url)), '..');

function trackedMarkdownFiles() {
    return execFileSync('git', ['ls-files', '-z', '--', '*.md'], {
        cwd: repositoryRoot,
        encoding: 'utf8',
    }).split('\0').filter(Boolean);
}

function maskCode(text) {
    let fence = null;
    return text.split('\n').map((line) => {
        const content = line.replace(/\r$/, '');
        const marker = content.match(/^ {0,3}(`{3,}|~{3,})(.*)$/);

        if (fence) {
            const closesFence = marker
                && marker[1][0] === fence.character
                && marker[1].length >= fence.length
                && marker[2].trim() === '';
            if (closesFence) fence = null;
            return ' '.repeat(line.length);
        }

        if (marker && !(marker[1][0] === '`' && marker[2].includes('`'))) {
            fence = { character: marker[1][0], length: marker[1].length };
            return ' '.repeat(line.length);
        }

        return line.replace(/(`+)(.*?)\1/g, match => ' '.repeat(match.length));
    }).join('\n');
}

function localPath(target, sourcePath) {
    const pathText = target.split(/[?#]/, 1)[0];
    if (!pathText || /^(?:[a-z][a-z\d+.-]*:|\/\/)/i.test(pathText)) return null;

    let decoded = pathText;
    try {
        decoded = decodeURIComponent(pathText);
    } catch {
        // A malformed escape remains a path and is reported as missing.
    }
    decoded = decoded.replace(/\\([\\`*{}\[\]()#+.!_>~-])/g, '$1');

    return decoded.startsWith('/')
        ? resolve(repositoryRoot, `.${decoded}`)
        : resolve(dirname(sourcePath), decoded);
}

function missingTarget(target, sourcePath) {
    const path = localPath(target, sourcePath);
    if (path === null) return false;

    const repositoryPath = relative(repositoryRoot, path);
    const outsideRepository = repositoryPath === '..'
        || repositoryPath.startsWith(`..${sep}`)
        || isAbsolute(repositoryPath);
    return outsideRepository || !existsSync(path);
}

function failuresIn(text, sourcePath) {
    const visible = maskCode(text);
    const failures = [];
    const patterns = [
        /!?\[(?:\\.|[^\]\\])*\]\(\s*(?:<([^>\r\n]+)>|([^\s)]+))/g,
        /^ {0,3}\[[^\]\r\n]+\]:[ \t]*(?:<([^>\r\n]+)>|([^\s]+))/gm,
        /<a\b[^>]*?\bhref\s*=\s*["']([^"']+)["']/gi,
        /<img\b[^>]*?\bsrc\s*=\s*["']([^"']+)["']/gi,
    ];

    for (const pattern of patterns) {
        for (const match of visible.matchAll(pattern)) {
            const target = (match[1] ?? match[2]).trim();
            if (target.startsWith('#') || !missingTarget(target, sourcePath)) continue;
            failures.push({
                line: visible.slice(0, match.index).split('\n').length,
                offset: match.index,
                target: target.replace(/\s+/g, ' '),
            });
        }
    }

    return failures.sort((left, right) => left.offset - right.offset);
}

let markdownFiles;
try {
    markdownFiles = trackedMarkdownFiles();
} catch (error) {
    const message = (error instanceof Error ? error.message : String(error)).split(/\r?\n/, 1)[0];
    console.error(`scripts/check-markdown-links.mjs:1: could not enumerate tracked Markdown: ${message}`);
    process.exit(1);
}

const failures = [];
for (const markdownFile of markdownFiles) {
    const sourcePath = resolve(repositoryRoot, markdownFile);
    let contents;
    try {
        contents = readFileSync(sourcePath, 'utf8');
    } catch (error) {
        failures.push(`${markdownFile}:1: could not read Markdown: ${error.message}`);
        continue;
    }

    for (const failure of failuresIn(contents, sourcePath)) {
        failures.push(`${markdownFile}:${failure.line}: local target "${failure.target}" does not exist.`);
    }
}

if (failures.length > 0) {
    console.error(failures.join('\n'));
    process.exit(1);
}

console.log(`Markdown link check passed: ${markdownFiles.length} tracked files checked.`);
