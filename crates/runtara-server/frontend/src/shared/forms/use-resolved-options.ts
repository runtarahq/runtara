import { useEffect, useMemo, useRef, useState } from 'react';

import type { FormDefinition, FormOption, OptionResolver } from './types';

export interface ResolvedOptionsState {
  options: Record<string, FormOption[]>;
  loading: ReadonlySet<string>;
  errors: Record<string, string>;
}

const EMPTY_STATE: ResolvedOptionsState = {
  options: {},
  loading: new Set<string>(),
  errors: {},
};

/**
 * Resolve dynamic choices declared by `control.optionResolver`.
 *
 * Only declared dependencies participate in the cache key. Requests are
 * aborted when their schema/dependencies change, so slow provider responses
 * cannot overwrite newer choices.
 */
export function useResolvedOptions(
  definition: FormDefinition,
  currentData: Record<string, unknown>,
  resolver?: OptionResolver
): ResolvedOptionsState {
  const request = useRef(0);
  const currentDataRef = useRef(currentData);
  currentDataRef.current = currentData;
  const definitionJson = JSON.stringify(definition);
  const requestsJson = useMemo(() => {
    if (!resolver) return '[]';
    const requests = Object.entries(definition.fields)
      .filter(([, field]) => Boolean(field.control?.optionResolver))
      .map(([fieldName, field]) => ({
        fieldName,
        resolverKey: field.control?.optionResolver,
        dependencies: Object.fromEntries(
          (field.control?.optionDependencies ?? []).map((name) => [
            name,
            currentData[name],
          ])
        ),
      }));
    return JSON.stringify(requests);
  }, [definition.fields, currentData, resolver]);
  const [state, setState] = useState<ResolvedOptionsState>(EMPTY_STATE);

  useEffect(() => {
    if (!resolver) {
      setState(EMPTY_STATE);
      return;
    }

    const definitionSnapshot = JSON.parse(definitionJson) as FormDefinition;
    const requests = JSON.parse(requestsJson) as Array<{
      fieldName: string;
      resolverKey: string;
    }>;
    if (requests.length === 0) {
      setState(EMPTY_STATE);
      return;
    }

    const current = ++request.current;
    const controller = new AbortController();
    const currentDataSnapshot = currentDataRef.current;
    setState({
      options: {},
      loading: new Set(requests.map(({ fieldName }) => fieldName)),
      errors: {},
    });

    void Promise.allSettled(
      requests.map(async ({ fieldName, resolverKey }) => ({
        fieldName,
        options: await resolver({
          resolverKey,
          fieldName,
          field: definitionSnapshot.fields[fieldName],
          currentData: currentDataSnapshot,
          signal: controller.signal,
        }),
      }))
    ).then((results) => {
      if (controller.signal.aborted || request.current !== current) return;

      const options: Record<string, FormOption[]> = {};
      const errors: Record<string, string> = {};
      results.forEach((result, index) => {
        const fieldName = requests[index].fieldName;
        if (result.status === 'fulfilled') {
          options[fieldName] = result.value.options;
        } else {
          errors[fieldName] =
            result.reason instanceof Error
              ? result.reason.message
              : 'Could not load options.';
        }
      });
      setState({ options, loading: new Set(), errors });
    });

    return () => controller.abort();
  }, [definitionJson, requestsJson, resolver]);

  return state;
}
