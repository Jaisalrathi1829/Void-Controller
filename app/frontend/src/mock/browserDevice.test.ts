import { describe, it, expect } from 'vitest';
import { BrowserSimulatedDevice } from './browserDevice';

describe('browser simulated device', () => {
  it('reveals nothing before connecting', async () => {
    const dev = new BrowserSimulatedDevice();
    const snap = await dev.snapshot();
    expect(snap.connection).toBe('disconnected');
    expect(snap.battery).toBeNull();
    expect(snap.identity).toBeNull();
    expect(snap.noiseControl).toBeNull();
    expect(snap.equalizer).toBeNull();
  });

  it('exposes simulated readings once connected', async () => {
    const dev = new BrowserSimulatedDevice();
    await dev.execute({ kind: 'connect' });
    const snap = await dev.snapshot();
    expect(snap.connection).toBe('connected');
    expect(snap.battery?.source).toBe('simulated');
    expect(snap.source).toBe('mock');
  });

  // The defining property: a simulation must never be able to convince the
  // application that a real capability was hardware-verified.
  it('never reports a hardware-verified capability', async () => {
    const dev = new BrowserSimulatedDevice();
    await dev.execute({ kind: 'connect' });
    const snap = await dev.snapshot();
    for (const cap of Object.values(snap.capabilities)) {
      expect(cap.hardwareVerified).toBe(false);
      expect(cap.status).not.toBe('verified');
    }
  });

  it('labels its identity as simulated', async () => {
    const dev = new BrowserSimulatedDevice();
    await dev.execute({ kind: 'connect' });
    const snap = await dev.snapshot();
    expect(snap.identity?.name.toUpperCase()).toContain('SIMULATED');
  });

  it('rejects commands when disconnected', async () => {
    const dev = new BrowserSimulatedDevice();
    const outcome = await dev.execute({ kind: 'setNoiseControl', mode: 'aware' });
    expect(outcome.kind).toBe('rejected');
  });

  it('rejects out-of-range EQ rather than clamping silently', async () => {
    const dev = new BrowserSimulatedDevice();
    await dev.execute({ kind: 'connect' });
    const outcome = await dev.execute({
      kind: 'setEqualizer',
      settings: { bass: 99, mid: 0, treble: 0 },
    });
    expect(outcome.kind).toBe('rejected');
  });

  it('applies and reports noise control changes', async () => {
    const dev = new BrowserSimulatedDevice();
    await dev.execute({ kind: 'connect' });
    const outcome = await dev.execute({ kind: 'setNoiseControl', mode: 'aware' });
    expect(outcome.kind).toBe('applied');
    const snap = await dev.snapshot();
    expect(snap.noiseControl?.mode).toBe('aware');
  });

  // Without this, the UI's "could not verify" path would never run in
  // development and would first be exercised against real hardware.
  it('exercises the unverified path periodically', async () => {
    const dev = new BrowserSimulatedDevice();
    await dev.execute({ kind: 'connect' });
    const kinds = new Set<string>();
    for (let i = 0; i < 14; i++) {
      const outcome = await dev.execute({ kind: 'setNoiseControl', mode: 'quiet' });
      kinds.add(outcome.kind);
    }
    expect(kinds.has('sent-unverified')).toBe(true);
  });

  it('reports windows audio as unsupported in the browser', async () => {
    const dev = new BrowserSimulatedDevice();
    await dev.execute({ kind: 'connect' });
    const outcome = await dev.execute({ kind: 'setSystemVolume', percent: 50 });
    expect(outcome.kind).toBe('unsupported');
  });

  it('clears exposed state on disconnect', async () => {
    const dev = new BrowserSimulatedDevice();
    await dev.execute({ kind: 'connect' });
    await dev.execute({ kind: 'disconnect' });
    const snap = await dev.snapshot();
    expect(snap.battery).toBeNull();
    expect(snap.identity).toBeNull();
  });
});

describe('the simulated devices to choose from', () => {
  it('lists headphones and earbuds, each with its model, all labelled simulated', async () => {
    const dev = new BrowserSimulatedDevice();
    const listed = await dev.paired();
    expect(listed.map((d) => d.model?.category)).toEqual([
      'headphones',
      'headphones',
      'earbuds',
      'headphones',
    ]);
    for (const d of listed) expect(d.name).toContain('(simulated)');
  });

  it('becomes the one chosen, with the modes and batteries of its model', async () => {
    const dev = new BrowserSimulatedDevice();
    const buds = (await dev.paired()).find((d) => d.model?.category === 'earbuds')!;
    const snap = await dev.choose(buds.id);
    expect(snap.connection).toBe('connected');
    expect(snap.model?.category).toBe('earbuds');
    expect(snap.noiseModes?.map((m) => m.name)).toEqual(['Quiet', 'Aware', 'Immersion', 'Cinema']);
    expect(snap.battery?.parts?.map((p) => p.name)).toEqual(['Right', 'Left', 'Case']);
    expect(snap.capabilities.customNoiseControl.status).toBe('unsupported');
    expect(snap.pairedDevices).toBeNull();
  });

  it('switches to a listed mode by its slot, and refuses one it does not list', async () => {
    const dev = new BrowserSimulatedDevice();
    const ultra = (await dev.paired()).find((d) => d.name.startsWith('Bose QC Ultra ('))!;
    await dev.choose(ultra.id);
    expect((await dev.execute({ kind: 'selectNoiseMode', index: 3 })).kind).toBe('applied');
    const snap = await dev.snapshot();
    expect(snap.noiseModes?.find((m) => m.active)?.name).toBe('Cinema');
    expect(snap.noiseControl?.label).toBe('Cinema');
    expect(snap.noiseControl?.level).toBeUndefined();
    expect((await dev.execute({ kind: 'selectNoiseMode', index: 9 })).kind).toBe('rejected');
  });

  it('refuses an id it did not list', async () => {
    const dev = new BrowserSimulatedDevice();
    await expect(dev.choose('ffffffffffffffffffff')).rejects.toMatchObject({ kind: 'not-connected' });
  });
});
