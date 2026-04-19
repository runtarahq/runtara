/* eslint-disable react-refresh/only-export-components */
import { useState, useEffect, useContext, useMemo } from 'react';
import { useMutation } from '@tanstack/react-query';
import { useFormContext } from 'react-hook-form';
import { shallow } from 'zustand/shallow';
import { useToken } from '@/shared/hooks';
import { Copy, CheckCircle, XCircle } from 'lucide-react';
import { Button } from '@/shared/components/ui/button';
import { Textarea } from '@/shared/components/ui/textarea';
import { Label } from '@/shared/components/ui/label';
import {
  Alert,
  AlertDescription,
  AlertTitle,
} from '@/shared/components/ui/alert';
import { useToast } from '@/shared/hooks/useToast';
import { testAgent } from '@/features/workflows/queries';
import { TestAgentResponse } from '@/generated/RuntaraRuntimeApi';
import { SimpleInputMappingEditor } from '../InputMappingField/SimpleInputMappingEditor';
import {
  useNodeFormStore,
  InputMappingEntry,
} from '@/features/workflows/stores/nodeFormStore';
import { NodeFormContext } from '../NodeFormContext';
import { parseTestAgentInputs } from '@/features/workflows/types/agent-metadata';

// Stable empty object to avoid creating new references on each render
const EMPTY_NODE_DATA: Record<string, InputMappingEntry> = {};

// Global ref to expose test functionality to parent components
interface TestHandler {
  runTest: () => void;
  isPending: boolean;
  isValid: boolean;
  isAvailable: boolean;
}

// Singleton to hold the current test handler
let currentTestHandler: TestHandler | null = null;

export function getTestHandler(): TestHandler | null {
  return currentTestHandler;
}

function setTestHandler(handler: TestHandler | null) {
  currentTestHandler = handler;
}

// Constant node ID for test form in Zustand store
const TEST_NODE_ID = '__test_agent__';

export function TestAgentInline() {
  const token = useToken();
  const { toast } = useToast();
  const { watch } = useFormContext();
  const { agents } = useContext(NodeFormContext);

  // Zustand store for test form values
  // Subscribe to the actual node data to trigger re-renders when it changes
  // Use shallow equality and stable empty object to prevent infinite re-renders
  const nodeData = useNodeFormStore(
    (s) => s.nodeInputMappings[TEST_NODE_ID] ?? EMPTY_NODE_DATA,
    shallow
  );
  const clearNode = useNodeFormStore((s) => s.clearNode);

  const [testResult, setTestResult] = useState<TestAgentResponse | null>(null);
  const [testError, setTestError] = useState<string | null>(null);

  // Watch form values
  const agentId = watch('agentId');
  const capabilityId = watch('capabilityId');
  const connectionId = watch('connectionId');
  const stepType = watch('stepType');

  // Get agent and capability metadata
  const agent = (agents as any[])?.find((ag) => ag.id === agentId);
  const capability = agent?.supportedCapabilities?.[capabilityId] as any;

  // Get the actual agent ID (lowercase) for API calls
  const actualAgentId = agent?.id;
  const hasEnhancedMetadata =
    capability &&
    Array.isArray(capability.inputs) &&
    capability.inputs.length > 0;

  // Filter inputs to remove CONNECTION_DATA fields
  // Memoize to prevent infinite re-render loops in SimpleInputMappingEditor
  const filteredInputs = useMemo(() => {
    if (!hasEnhancedMetadata) return [];
    return capability.inputs.filter(
      (field: any) => !field.name.startsWith('get__CONNECTION_DATA')
    );
  }, [hasEnhancedMetadata, capability?.inputs]);

  // Clear test form data and reset results when capability changes
  useEffect(() => {
    clearNode(TEST_NODE_ID);
    setTestResult(null);
    setTestError(null);
  }, [capabilityId, clearNode]);

  // Cleanup on unmount
  useEffect(() => {
    return () => {
      clearNode(TEST_NODE_ID);
    };
  }, [clearNode]);

  // Validate required fields
  const validateInputs = (): boolean => {
    for (const field of filteredInputs) {
      if (field.required) {
        const entry = nodeData[field.name];
        const value = entry?.value;
        if (value === undefined || value === null || value === '') {
          return false;
        }
      }
    }
    return true;
  };

  const testMutation = useMutation({
    mutationFn: async (inputs: Record<string, any>) => {
      if (!actualAgentId) {
        throw new Error('Agent not found');
      }
      return testAgent(
        token,
        actualAgentId,
        capabilityId,
        inputs,
        connectionId
      );
    },
    onSuccess: (data) => {
      if (data.success) {
        setTestResult(data);
        setTestError(null);
      } else {
        setTestError(data.error || 'Test failed with no error message');
        setTestResult(data);
      }
    },
    onError: (error: any) => {
      let errorMessage =
        error?.error || error?.message || 'Failed to test agent';

      // Handle rate limiting with a friendly message
      if (error?.response?.status === 429 || errorMessage.includes('429')) {
        errorMessage =
          "Whoa there, speed racer! 🏎️ You're testing too fast. Take a quick breather and try again in a moment.";
      }

      setTestError(errorMessage);
      setTestResult(null);
    },
  });

  const handleRunTest = async () => {
    if (!validateInputs()) {
      return;
    }
    // Get values from Zustand store
    const formValues: Record<string, any> = {};
    Object.entries(nodeData).forEach(([fieldName, entry]) => {
      formValues[fieldName] = entry.value;
    });
    // Parse form values to convert JSON strings to proper types
    const parsedInputs = hasEnhancedMetadata
      ? parseTestAgentInputs(formValues, filteredInputs)
      : formValues;
    testMutation.mutate(parsedInputs);
  };

  const handleCopyOutput = () => {
    if (testResult?.output) {
      const outputText =
        typeof testResult.output === 'string'
          ? testResult.output
          : JSON.stringify(testResult.output, null, 2);
      navigator.clipboard.writeText(outputText);
      toast({
        title: 'Copied',
        description: 'Output copied to clipboard',
      });
    }
  };

  // Check if agent testing is available
  const isAvailable = stepType === 'Agent' && !!agentId && !!capabilityId;

  // Register the test handler for external access
  useEffect(() => {
    setTestHandler({
      runTest: handleRunTest,
      isPending: testMutation.isPending,
      isValid: validateInputs(),
      isAvailable,
    });
    return () => setTestHandler(null);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [testMutation.isPending, isAvailable, nodeData]);

  if (!isAvailable) {
    return (
      <div className="text-center text-muted-foreground py-8">
        Select an agent and capability to test
      </div>
    );
  }

  // Remove this check - capabilities with no inputs can still be tested
  // The test form will just be empty, which is valid

  return (
    <div className="space-y-6">
      {/* Input Form Section */}
      {filteredInputs.length > 0 ? (
        <SimpleInputMappingEditor
          nodeId={TEST_NODE_ID}
          fields={filteredInputs}
          hideReferenceToggle
        />
      ) : (
        <div className="text-center text-muted-foreground py-4 text-sm">
          This capability requires no input parameters
        </div>
      )}

      {/* Results Section */}
      {(testResult || testError) && (
        <div className="space-y-4">
          <div className="flex items-center gap-2">
            {testError ? (
              <>
                <XCircle className="h-5 w-5 text-destructive" />
                <h3 className="text-sm font-semibold">Test Failed</h3>
              </>
            ) : (
              <>
                <CheckCircle className="h-5 w-5 text-green-600" />
                <h3 className="text-sm font-semibold">Test Successful</h3>
              </>
            )}
          </div>

          {testError ? (
            <>
              <Alert variant="destructive">
                <AlertTitle>Error</AlertTitle>
                <AlertDescription>
                  <Textarea
                    value={testError}
                    readOnly
                    className="font-mono text-sm min-h-[150px] max-h-[300px] mt-2 bg-destructive/5 border-destructive/20 text-destructive resize-none"
                  />
                </AlertDescription>
              </Alert>

              {testResult && (
                <div className="grid grid-cols-2 gap-4">
                  {testResult.executionTimeMs != null && (
                    <div className="rounded-lg bg-muted p-3">
                      <p className="text-xs text-muted-foreground mb-1">
                        Execution Time
                      </p>
                      <p className="text-lg font-semibold">
                        {testResult.executionTimeMs.toFixed(2)} ms
                      </p>
                    </div>
                  )}
                  {testResult.maxMemoryMb != null && (
                    <div className="rounded-lg bg-muted p-3">
                      <p className="text-xs text-muted-foreground mb-1">
                        Memory Used
                      </p>
                      <p className="text-lg font-semibold">
                        {testResult.maxMemoryMb.toFixed(2)} MB
                      </p>
                    </div>
                  )}
                </div>
              )}
            </>
          ) : (
            <>
              <div>
                <div className="flex items-center justify-between mb-2">
                  <Label>Output</Label>
                  <Button
                    type="button"
                    variant="ghost"
                    size="sm"
                    onClick={handleCopyOutput}
                    className="h-8"
                  >
                    <Copy className="h-3 w-3 mr-1" />
                    Copy
                  </Button>
                </div>
                <Textarea
                  value={
                    testResult?.output
                      ? typeof testResult.output === 'string'
                        ? testResult.output
                        : JSON.stringify(testResult.output, null, 2)
                      : 'No output'
                  }
                  readOnly
                  className="font-mono text-sm min-h-[200px]"
                />
              </div>

              <div className="grid grid-cols-2 gap-4">
                {testResult?.executionTimeMs != null && (
                  <div className="rounded-lg bg-muted p-3">
                    <p className="text-xs text-muted-foreground mb-1">
                      Execution Time
                    </p>
                    <p className="text-lg font-semibold">
                      {testResult.executionTimeMs.toFixed(2)} ms
                    </p>
                  </div>
                )}
                {testResult?.maxMemoryMb != null && (
                  <div className="rounded-lg bg-muted p-3">
                    <p className="text-xs text-muted-foreground mb-1">
                      Memory Used
                    </p>
                    <p className="text-lg font-semibold">
                      {testResult.maxMemoryMb.toFixed(2)} MB
                    </p>
                  </div>
                )}
              </div>
            </>
          )}
        </div>
      )}
    </div>
  );
}
