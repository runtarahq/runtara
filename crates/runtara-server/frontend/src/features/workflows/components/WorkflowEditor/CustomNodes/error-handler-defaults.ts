/**
 * Inputs a new error-handler (Error) step starts with, so the route is valid
 * the moment it is added: a stable code, a message, and a permanent error.
 */
export function defaultErrorHandlerInputMapping(): Record<string, unknown>[] {
  return [
    {
      type: 'code',
      value: 'HANDLED_ERROR',
      typeHint: 'string',
      valueType: 'immediate',
    },
    {
      type: 'message',
      value: 'Handled by workflow error route',
      typeHint: 'string',
      valueType: 'immediate',
    },
    {
      type: 'category',
      value: 'permanent',
      typeHint: 'string',
      valueType: 'immediate',
    },
    {
      type: 'severity',
      value: 'error',
      typeHint: 'string',
      valueType: 'immediate',
    },
  ];
}
