import { z } from 'zod';

// Schema is now empty since name is no longer editable in the form
// The form wrapper is kept for the submit action
export const schema = z.object({});

export const initialValues = {};
