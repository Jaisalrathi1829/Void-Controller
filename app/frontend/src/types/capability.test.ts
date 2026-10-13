import { describe, it, expect } from 'vitest';
import {
  capabilityTransition,
  createUnknownCapabilities,
  CapabilityTransitionError,
  isActionable,
  requiresUnverifiedWarning,
  CAPABILITY_KEYS,
  type CapabilityRecord,
} from './capability';

function record(overrides: Partial<CapabilityRecord> = {}): CapabilityRecord {
  return {
    key: 'battery',
    status: 'unknown',
    mechanism: 'none',
    hardwareVerified: false,
    evidence: 'initial',
    ...overrides,
  };
}

describe('capability defaults', () => {
  it('starts every capability unknown and unverified', () => {
    const caps = createUnknownCapabilities();
    expect(Object.keys(caps)).toHaveLength(CAPABILITY_KEYS.length);
    for (const key of CAPABILITY_KEYS) {
      expect(caps[key].status).toBe('unknown');
      expect(caps[key].hardwareVerified).toBe(false);
    }
  });
});

describe('capability transitions', () => {
  it('requires evidence for every transition', () => {
    expect(() => capabilityTransition(record(), 'supported', '')).toThrow(
      CapabilityTransitionError,
    );
    expect(() => capabilityTransition(record(), 'supported', '   ')).toThrow(
      CapabilityTransitionError,
    );
  });

  it('refuses to reach verified without hardware evidence', () => {
    expect(() =>
      capabilityTransition(record(), 'verified', 'the command was accepted'),
    ).toThrow(CapabilityTransitionError);
  });

  it('reaches verified only with explicit hardware evidence', () => {
    const next = capabilityTransition(record(), 'verified', 'device reported the new mode', {
      hardwareEvidence: true,
    });
    expect(next.status).toBe('verified');
    expect(next.hardwareVerified).toBe(true);
  });

  // The central guarantee: an interface existing is not the device working.
  it('never sets hardwareVerified when moving to supported', () => {
    const next = capabilityTransition(record(), 'supported', 'GATT characteristic exists');
    expect(next.status).toBe('supported');
    expect(next.hardwareVerified).toBe(false);
  });

  it('treats unsupported as terminal within a session', () => {
    const unsupported = record({ status: 'unsupported' });
    expect(() =>
      capabilityTransition(unsupported, 'supported', 'found something after all'),
    ).toThrow(CapabilityTransitionError);
  });

  it('stamps lastEvaluated on transition', () => {
    const next = capabilityTransition(record(), 'ready', 'partial evidence');
    expect(next.lastEvaluated).toBeTruthy();
    expect(() => new Date(next.lastEvaluated!).toISOString()).not.toThrow();
  });
});

describe('actionability semantics', () => {
  it('treats only verified as actionable without caveat', () => {
    expect(isActionable(record({ status: 'verified' }))).toBe(true);
    expect(isActionable(record({ status: 'supported' }))).toBe(false);
    expect(isActionable(record({ status: 'ready' }))).toBe(false);
    expect(isActionable(record({ status: 'unknown' }))).toBe(false);
    expect(isActionable(record({ status: 'unsupported' }))).toBe(false);
  });

  it('requires a caveat for supported and ready', () => {
    expect(requiresUnverifiedWarning(record({ status: 'supported' }))).toBe(true);
    expect(requiresUnverifiedWarning(record({ status: 'ready' }))).toBe(true);
    expect(requiresUnverifiedWarning(record({ status: 'verified' }))).toBe(false);
  });
});
