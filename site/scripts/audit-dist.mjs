import { readdir, readFile } from 'node:fs/promises';
import { extname, join, relative } from 'node:path';
import process from 'node:process';

const root = join(process.cwd(), 'dist');
const files = [];

async function walk(directory) {
  for (const entry of await readdir(directory, { withFileTypes: true })) {
    const path = join(directory, entry.name);
    if (entry.isDirectory()) await walk(path);
    else files.push(path);
  }
}

await walk(root);

const forbiddenFiles = /(?:^|\/)\.env(?:\..*)?$|\.(?:astro|map|mdx?|svelte|tsx?)$/i;
const textExtensions = new Set(['.css', '.html', '.js', '.json', '.svg', '.txt', '.xml']);
const forbiddenText = [
  ['private key', /BEGIN (?:RSA |EC |OPENSSH )?PRIVATE KEY/i],
  ['GitHub token', /(?:github_pat_|gh[pousr]_)[A-Za-z0-9_]{20,}/],
  ['OpenAI-style key', /sk-[A-Za-z0-9_-]{20,}/],
  ['AWS access key', /AKIA[0-9A-Z]{16}/],
  ['Cloudflare token', /CLOUDFLARE_(?:API_)?TOKEN/i],
  ['source map reference', /sourceMappingURL=/],
  ['local macOS path', /\/Users\/[^<&\s"']+/],
  ['repository path', /Documents\/Code/i],
];

const failures = [];

for (const file of files) {
  const name = relative(root, file);
  if (forbiddenFiles.test(name)) failures.push(`${name}: forbidden output file`);
  if (!textExtensions.has(extname(file).toLowerCase())) continue;

  let body = await readFile(file, 'utf8');
  body = body
    .replaceAll('/Users/<your-user>', '')
    .replaceAll('/Users/&#x3C;your-user>', '');

  for (const [label, pattern] of forbiddenText) {
    if (pattern.test(body)) failures.push(`${name}: ${label}`);
  }
}

if (failures.length) {
  console.error('Public output audit failed:');
  for (const failure of failures) console.error(`- ${failure}`);
  process.exitCode = 1;
} else {
  console.log(`Public output audit passed (${files.length} files).`);
}
