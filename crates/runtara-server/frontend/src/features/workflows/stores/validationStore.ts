import { create } from 'zustand';
import { devtools } from 'zustand/middleware';
import { immer } from 'zustand/middleware/immer';
import { ValidationMessage, ValidationFilter } from '../types/validation';

/** Available tabs in the bottom panel */
export type BottomPanelTab = 'problems' | 'history' | 'settings' | 'versions';

interface ValidationState {
  /** All validation messages */
  messages: ValidationMessage[];

  /** Set of step IDs with errors (for quick lookup and node highlighting) */
  stepsWithErrors: Set<string>;

  /** Set of step IDs with warnings (for quick lookup and node highlighting) */
  stepsWithWarnings: Set<string>;

  /** Whether the panel is expanded */
  isPanelExpanded: boolean;

  /** Current active tab in the bottom panel */
  activeTab: BottomPanelTab;

  /** Current filter for problems tab */
  activeFilter: ValidationFilter;

  /** Last validation timestamp */
  lastValidatedAt: number | null;
}

interface ValidationActions {
  /** Set all validation messages (replaces existing) */
  setMessages: (messages: ValidationMessage[]) => void;

  /** Add messages to existing ones */
  addMessages: (messages: ValidationMessage[]) => void;

  /** Clear messages, optionally by source */
  clearMessages: (source?: 'client' | 'server') => void;

  /** Set panel expanded state */
  setPanelExpanded: (expanded: boolean) => void;

  /** Toggle panel expanded state */
  togglePanel: () => void;

  /** Set active tab in the bottom panel */
  setActiveTab: (tab: BottomPanelTab) => void;

  /** Set active filter for problems tab */
  setActiveFilter: (filter: ValidationFilter) => void;

  /** Get error count */
  getErrorCount: () => number;

  /** Get warning count */
  getWarningCount: () => number;

  /** Get filtered messages based on active filter */
  getFilteredMessages: () => ValidationMessage[];

  /** Get first error step ID for navigation */
  getFirstErrorStepId: () => string | null;
}

type ValidationStore = ValidationState & ValidationActions;

/**
 * Rebuild the step sets from messages array
 */
function rebuildStepSets(messages: ValidationMessage[]): {
  stepsWithErrors: Set<string>;
  stepsWithWarnings: Set<string>;
} {
  const stepsWithErrors = new Set<string>();
  const stepsWithWarnings = new Set<string>();

  messages.forEach((msg) => {
    if (msg.stepId) {
      if (msg.severity === 'error') {
        stepsWithErrors.add(msg.stepId);
      } else {
        stepsWithWarnings.add(msg.stepId);
      }
    }
    // Also add related step IDs
    msg.relatedStepIds?.forEach((id) => {
      if (msg.severity === 'error') {
        stepsWithErrors.add(id);
      } else {
        stepsWithWarnings.add(id);
      }
    });
  });

  return { stepsWithErrors, stepsWithWarnings };
}

/**
 * Selector: get the first validation error message for a specific step.
 * Usage: useValidationStore(state => getFirstValidationMessage(state, stepId))
 */
export function getFirstValidationMessage(
  state: Pick<ValidationState, 'messages'>,
  stepId: string
): string | null {
  const msg = state.messages.find(
    (m) => m.stepId === stepId && m.severity === 'error'
  );
  return msg?.message ?? null;
}

export const useValidationStore = create<ValidationStore>()(
  devtools(
    immer((set, get) => ({
      // Initial state
      messages: [],
      stepsWithErrors: new Set<string>(),
      stepsWithWarnings: new Set<string>(),
      isPanelExpanded: false,
      activeTab: 'problems',
      activeFilter: 'all',
      lastValidatedAt: null,

      // Actions
      setMessages: (messages) =>
        set((state) => {
          state.messages = messages;
          state.lastValidatedAt = Date.now();

          // Rebuild step sets
          const { stepsWithErrors, stepsWithWarnings } =
            rebuildStepSets(messages);
          state.stepsWithErrors = stepsWithErrors;
          state.stepsWithWarnings = stepsWithWarnings;

          // Auto-expand panel and switch to problems tab if there are new errors
          if (messages.some((m) => m.severity === 'error')) {
            state.isPanelExpanded = true;
            state.activeTab = 'problems';
          }
        }),

      addMessages: (newMessages) =>
        set((state) => {
          state.messages.push(...newMessages);
          state.lastValidatedAt = Date.now();

          // Update step sets
          newMessages.forEach((msg) => {
            if (msg.stepId) {
              if (msg.severity === 'error') {
                state.stepsWithErrors.add(msg.stepId);
              } else {
                state.stepsWithWarnings.add(msg.stepId);
              }
            }
            msg.relatedStepIds?.forEach((id) => {
              if (msg.severity === 'error') {
                state.stepsWithErrors.add(id);
              } else {
                state.stepsWithWarnings.add(id);
              }
            });
          });

          // Auto-expand panel and switch to problems tab if there are new errors
          if (newMessages.some((m) => m.severity === 'error')) {
            state.isPanelExpanded = true;
            state.activeTab = 'problems';
          }
        }),

      clearMessages: (source) =>
        set((state) => {
          if (source) {
            state.messages = state.messages.filter((m) => m.source !== source);
          } else {
            state.messages = [];
          }

          // Rebuild step sets from remaining messages
          const { stepsWithErrors, stepsWithWarnings } = rebuildStepSets(
            state.messages
          );
          state.stepsWithErrors = stepsWithErrors;
          state.stepsWithWarnings = stepsWithWarnings;
        }),

      setPanelExpanded: (expanded) =>
        set((state) => {
          state.isPanelExpanded = expanded;
        }),

      togglePanel: () =>
        set((state) => {
          state.isPanelExpanded = !state.isPanelExpanded;
        }),

      setActiveTab: (tab) =>
        set((state) => {
          state.activeTab = tab;
          // Expand panel when switching tabs
          if (!state.isPanelExpanded) {
            state.isPanelExpanded = true;
          }
        }),

      setActiveFilter: (filter) =>
        set((state) => {
          state.activeFilter = filter;
        }),

      // Computed helpers
      getErrorCount: () => {
        return get().messages.filter((m) => m.severity === 'error').length;
      },

      getWarningCount: () => {
        return get().messages.filter((m) => m.severity === 'warning').length;
      },

      getFilteredMessages: () => {
        const { messages, activeFilter } = get();
        if (activeFilter === 'all') {
          // Sort: errors first, then warnings, then by timestamp
          return [...messages].sort((a, b) => {
            if (a.severity !== b.severity) {
              return a.severity === 'error' ? -1 : 1;
            }
            return a.timestamp - b.timestamp;
          });
        }
        return messages.filter((m) =>
          activeFilter === 'errors'
            ? m.severity === 'error'
            : m.severity === 'warning'
        );
      },

      getFirstErrorStepId: () => {
        const firstError = get().messages.find(
          (m) => m.severity === 'error' && m.stepId
        );
        return firstError?.stepId || null;
      },
    })),
    { name: 'validation-store' }
  )
);
