import { describe, expect, it } from 'vitest';
import { extractReleaseNotes } from '../../scripts/release-notes.mjs';

describe('release notes extraction', () => {
  const changelog = `# Changelog

## [0.1.2] - 2026-08-10

### Changed

- Simplified connection timeout diagnostics.

## [0.1.1] - 2026-08-10

### Added

- Added release packages.
`;

  it('returns only the notes for the tagged version', () => {
    expect(extractReleaseNotes(changelog, 'v0.1.2')).toBe('### Changed\n\n- Simplified connection timeout diagnostics.');
  });

  it('fails when the tag has no changelog entry', () => {
    expect(() => extractReleaseNotes(changelog, 'v0.1.3')).toThrow('Missing changelog entry for v0.1.3');
  });
});
