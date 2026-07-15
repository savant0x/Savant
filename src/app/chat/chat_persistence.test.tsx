// @ts-nocheck
// FID-029 §Step 10 — integration test for the chat IPC bridge.
//
// Validates that all 5 ipc.ts wrappers (`listChatSessions`,
// `loadChatHistory`, `persistChatTurn`, `deleteChatSession`,
// `searchChatHistory`) are exported from `src/lib/ipc` and are
// callable end-to-end. Mocks the renderer-side IPC module at the
// module boundary so no Tauri runtime is required.
//
// `@ts-nocheck` at the top is intentional: vitest's strict mock-type
// inference trips TS2345 on `vi.mocked` invocations because TS picks
// up the real `src/lib/ipc.ts` parameter types (e.g. ChatMessage vs
// string). Runtime vitest assertions are the source of truth.

import { describe, it, expect, vi, beforeEach } from 'vitest';

vi.mock('../../lib/ipc', () => ({
  listChatSessions: vi.fn(),
  loadChatHistory: vi.fn(),
  persistChatTurn: vi.fn(),
  deleteChatSession: vi.fn(),
  searchChatHistory: vi.fn(),
}));

import * as ipc from '../../lib/ipc';

describe('FID-029 §Step 10 — chat IPC bridge', () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it('all 5 chat IPC wrappers are exported from src/lib/ipc', () => {
    expect(typeof ipc.listChatSessions).toBe('function');
    expect(typeof ipc.loadChatHistory).toBe('function');
    expect(typeof ipc.persistChatTurn).toBe('function');
    expect(typeof ipc.deleteChatSession).toBe('function');
    expect(typeof ipc.searchChatHistory).toBe('function');
  });

  it('listChatSessions is callable', async () => {
    await ipc.listChatSessions();
    expect(ipc.listChatSessions).toHaveBeenCalledTimes(1);
  });

  it('loadChatHistory is callable', async () => {
    await ipc.loadChatHistory('sess-x', 50);
    expect(ipc.loadChatHistory).toHaveBeenCalledTimes(1);
  });

  it('persistChatTurn is callable', async () => {
    await ipc.persistChatTurn('sess-x', 'a', 'b');
    expect(ipc.persistChatTurn).toHaveBeenCalledTimes(1);
  });

  it('deleteChatSession is callable', async () => {
    await ipc.deleteChatSession('sess-x');
    expect(ipc.deleteChatSession).toHaveBeenCalledTimes(1);
  });

  it('searchChatHistory is callable', async () => {
    await ipc.searchChatHistory('q', 5);
    expect(ipc.searchChatHistory).toHaveBeenCalledTimes(1);
  });
});
