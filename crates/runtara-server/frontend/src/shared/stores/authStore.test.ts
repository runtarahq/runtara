import { beforeEach, describe, expect, it } from 'vitest';
import { useAuthStore } from './authStore.ts';

describe('authStore', () => {
  // Reset the store before each test
  beforeEach(() => {
    useAuthStore.setState({ userGroups: [] });
  });

  it('initializes with empty user groups', () => {
    const state = useAuthStore.getState();
    expect(state.userGroups).toEqual([]);
  });

  it('sets user groups correctly', () => {
    const groups = ['admin', 'user', 'editor'];

    // Call the action to set user groups
    useAuthStore.getState().setUserGroups(groups);

    // Check if the state was updated correctly
    const updatedState = useAuthStore.getState();
    expect(updatedState.userGroups).toEqual(groups);
    expect(updatedState.userGroups).toHaveLength(3);
    expect(updatedState.userGroups).toContain('admin');
    expect(updatedState.userGroups).toContain('user');
    expect(updatedState.userGroups).toContain('editor');
  });

  it('clears user groups correctly', () => {
    // First set some user groups
    const groups = ['admin', 'user'];
    useAuthStore.getState().setUserGroups(groups);

    // Verify groups were set
    let state = useAuthStore.getState();
    expect(state.userGroups).toEqual(groups);

    // Call the action to clear user groups
    useAuthStore.getState().clearUserGroups();

    // Check if the state was updated correctly
    state = useAuthStore.getState();
    expect(state.userGroups).toEqual([]);
    expect(state.userGroups).toHaveLength(0);
  });

  it('replaces existing user groups when setting new ones', () => {
    // First set some user groups
    const initialGroups = ['admin', 'user'];
    useAuthStore.getState().setUserGroups(initialGroups);

    // Verify initial groups were set
    let state = useAuthStore.getState();
    expect(state.userGroups).toEqual(initialGroups);

    // Set new groups
    const newGroups = ['editor', 'viewer'];
    useAuthStore.getState().setUserGroups(newGroups);

    // Check if the state was updated correctly
    state = useAuthStore.getState();
    expect(state.userGroups).toEqual(newGroups);
    expect(state.userGroups).toHaveLength(2);
    expect(state.userGroups).toContain('editor');
    expect(state.userGroups).toContain('viewer');
    expect(state.userGroups).not.toContain('admin');
    expect(state.userGroups).not.toContain('user');
  });
});
