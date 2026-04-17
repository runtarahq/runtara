# Testing Guide for Runtara Frontend

This guide provides information on how to write, run, and maintain tests for the Runtara frontend application.

## Running Tests

### Basic Test Commands

- Run all tests once:

  ```bash
  npm test
  ```

- Run tests in watch mode (for development):

  ```bash
  npm run test:watch
  ```

- Run tests with coverage report:
  ```bash
  npm run test:coverage
  ```

## Writing Tests

### Test File Naming and Location

- Test files should be placed next to the files they test
- Use the `.test.ts` or `.test.tsx` extension
- Example: `src/components/modal-dialog.tsx` → `src/components/modal-dialog.test.tsx`

### Testing Components

Use React Testing Library to test components:

```typescript
import { render, screen, fireEvent } from '@testing-library/react';
import { MyComponent } from './MyComponent';

describe('MyComponent', () => {
  it('renders correctly', () => {
    render(<MyComponent />);
    expect(screen.getByText('Expected Text')).toBeInTheDocument();
  });

  it('handles user interactions', () => {
    const onClickMock = vi.fn();
    render(<MyComponent onClick={onClickMock} />);

    fireEvent.click(screen.getByRole('button'));

    expect(onClickMock).toHaveBeenCalledTimes(1);
  });
});
```

### Testing Hooks

Use `renderHook` from React Testing Library to test custom hooks:

```typescript
import { renderHook, act } from '@testing-library/react';
import { useMyHook } from './useMyHook';

describe('useMyHook', () => {
  it('initializes with default values', () => {
    const { result } = renderHook(() => useMyHook());
    expect(result.current.value).toBe(defaultValue);
  });

  it('updates state correctly', () => {
    const { result } = renderHook(() => useMyHook());

    act(() => {
      result.current.setValue('new value');
    });

    expect(result.current.value).toBe('new value');
  });
});
```

### Testing Zustand Stores

Test Zustand stores by directly accessing the store:

```typescript
import { useMyStore } from './myStore';

describe('myStore', () => {
  beforeEach(() => {
    // Reset the store before each test
    useMyStore.setState({ value: initialValue });
  });

  it('initializes with default values', () => {
    const state = useMyStore.getState();
    expect(state.value).toBe(initialValue);
  });

  it('updates state correctly', () => {
    useMyStore.getState().setValue('new value');

    const state = useMyStore.getState();
    expect(state.value).toBe('new value');
  });
});
```

## Testing Best Practices

### What to Test

1. **Components**

   - Rendering with different props
   - User interactions (clicks, inputs, etc.)
   - Conditional rendering
   - Error states

2. **Hooks**

   - Initialization with default values
   - State updates
   - Side effects
   - Error handling

3. **Stores**
   - Initial state
   - State updates through actions
   - Complex state transformations

### Test Coverage

- Aim for high test coverage, but prioritize critical paths
- Focus on testing business logic and user interactions
- Use coverage reports to identify gaps

### Mocking

- Mock external dependencies (APIs, browser APIs, etc.)
- Use Vitest's mocking capabilities:

  ```typescript
  import { vi } from 'vitest';

  // Mock a function
  const mockFn = vi.fn();

  // Mock a module
  vi.mock('./myModule', () => ({
    myFunction: vi.fn(),
  }));
  ```

## Example Tests

The project includes several example tests that demonstrate best practices:

- Component test: `src/components/modal-dialog.test.tsx`
- Hook test: `src/hooks/usePagination.test.ts`
- Store test: `src/stores/authStore.test.ts`

Use these as templates when writing new tests.

## Improving Test Coverage

See the [Test Coverage Gaps Report](./coverage-report.md) for a detailed analysis of test coverage gaps and recommendations for improvement.
