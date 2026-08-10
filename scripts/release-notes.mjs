import { readFileSync, writeFileSync } from 'node:fs';

/**
 * 从变更记录中提取指定标签对应的发布说明。
 * @param {string} changelog 变更记录全文。
 * @param {string} tag Git 标签，例如 v0.1.2。
 * @returns {string} 不包含版本标题的发布说明。
 */
export function extractReleaseNotes(changelog, tag) {
  const version = tag.startsWith('v') ? tag.slice(1) : tag;
  const lines = changelog.split('\n');
  const start = lines.findIndex((line) => line.startsWith(`## [${version}]`));

  if (start === -1) throw new Error(`Missing changelog entry for ${tag}`);

  const end = lines.slice(start + 1).findIndex((line) => line.startsWith('## ['));
  const notes = lines.slice(start + 1, end === -1 ? undefined : start + end + 1).join('\n').trim();

  if (!notes) throw new Error(`Empty changelog entry for ${tag}`);
  return notes;
}

if (process.argv[1] === new URL(import.meta.url).pathname) {
  const [, , tag, changelogPath, outputPath] = process.argv;
  writeFileSync(outputPath, `${extractReleaseNotes(readFileSync(changelogPath, 'utf8'), tag)}\n`);
}
