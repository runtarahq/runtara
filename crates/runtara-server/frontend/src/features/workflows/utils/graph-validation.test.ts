import { describe, it, expect } from 'vitest';
import type { Edge, Node } from '@xyflow/react';
import {
  isSelfConnection,
  wouldCreateLoop,
  validateConnection,
  validateWorkflowStructure,
} from './graph-validation';

describe('graph-validation', () => {
  describe('isSelfConnection', () => {
    it('should return true when source equals target', () => {
      expect(isSelfConnection('node-1', 'node-1')).toBe(true);
    });

    it('should return false when source and target are different', () => {
      expect(isSelfConnection('node-1', 'node-2')).toBe(false);
    });
  });

  describe('wouldCreateLoop', () => {
    it('should return false for empty graph', () => {
      const edges: Edge[] = [];
      expect(wouldCreateLoop(edges, 'A', 'B')).toBe(false);
    });

    it('should return false for linear connections', () => {
      const edges: Edge[] = [
        { id: 'e1', source: 'A', target: 'B' },
        { id: 'e2', source: 'B', target: 'C' },
      ];
      expect(wouldCreateLoop(edges, 'C', 'D')).toBe(false);
    });

    it('should detect simple loop (A→B, trying to add B→A)', () => {
      const edges: Edge[] = [{ id: 'e1', source: 'A', target: 'B' }];
      expect(wouldCreateLoop(edges, 'B', 'A')).toBe(true);
    });

    it('should detect loop with 3 nodes (A→B→C, trying to add C→A)', () => {
      const edges: Edge[] = [
        { id: 'e1', source: 'A', target: 'B' },
        { id: 'e2', source: 'B', target: 'C' },
      ];
      expect(wouldCreateLoop(edges, 'C', 'A')).toBe(true);
    });

    it('should detect loop with 4 nodes (A→B→C→D, trying to add D→A)', () => {
      const edges: Edge[] = [
        { id: 'e1', source: 'A', target: 'B' },
        { id: 'e2', source: 'B', target: 'C' },
        { id: 'e3', source: 'C', target: 'D' },
      ];
      expect(wouldCreateLoop(edges, 'D', 'A')).toBe(true);
    });

    it('should detect loop in middle of chain (A→B→C→D, trying to add C→B)', () => {
      const edges: Edge[] = [
        { id: 'e1', source: 'A', target: 'B' },
        { id: 'e2', source: 'B', target: 'C' },
        { id: 'e3', source: 'C', target: 'D' },
      ];
      expect(wouldCreateLoop(edges, 'C', 'B')).toBe(true);
    });

    it('should allow multiple paths to same node (A→B, A→C, B→D, C→D)', () => {
      const edges: Edge[] = [
        { id: 'e1', source: 'A', target: 'B' },
        { id: 'e2', source: 'A', target: 'C' },
        { id: 'e3', source: 'B', target: 'D' },
      ];
      // Adding C→D should be allowed (diamond pattern)
      expect(wouldCreateLoop(edges, 'C', 'D')).toBe(false);
    });

    it('should handle complex graph with branches', () => {
      const edges: Edge[] = [
        { id: 'e1', source: 'A', target: 'B' },
        { id: 'e2', source: 'A', target: 'C' },
        { id: 'e3', source: 'B', target: 'D' },
        { id: 'e4', source: 'C', target: 'E' },
        { id: 'e5', source: 'D', target: 'F' },
        { id: 'e6', source: 'E', target: 'F' },
      ];
      // Adding F→A would create a loop
      expect(wouldCreateLoop(edges, 'F', 'A')).toBe(true);
      // Adding F→G would not create a loop
      expect(wouldCreateLoop(edges, 'F', 'G')).toBe(false);
    });

    it('should handle disconnected subgraphs', () => {
      const edges: Edge[] = [
        { id: 'e1', source: 'A', target: 'B' },
        { id: 'e2', source: 'C', target: 'D' },
      ];
      // Connecting between disconnected graphs should not create loop
      expect(wouldCreateLoop(edges, 'B', 'C')).toBe(false);
      expect(wouldCreateLoop(edges, 'D', 'A')).toBe(false);
    });

    it('should handle graph with multiple source handles (conditional nodes)', () => {
      const edges: Edge[] = [
        { id: 'e1', source: 'A', target: 'B', sourceHandle: 'true' },
        { id: 'e2', source: 'A', target: 'C', sourceHandle: 'false' },
        { id: 'e3', source: 'B', target: 'D' },
        { id: 'e4', source: 'C', target: 'D' },
      ];
      // Adding D→A would create a loop regardless of handle
      expect(wouldCreateLoop(edges, 'D', 'A')).toBe(true);
    });
  });

  describe('validateConnection', () => {
    it('should reject self-connection', () => {
      const edges: Edge[] = [];
      const nodes: Node[] = [];
      const result = validateConnection(edges, nodes, 'A', 'A');
      expect(result.isValid).toBe(false);
      expect(result.errorMessage).toBe('A step cannot connect to itself');
    });

    it('should reject connection that would create loop', () => {
      const edges: Edge[] = [
        { id: 'e1', source: 'A', target: 'B' },
        { id: 'e2', source: 'B', target: 'C' },
      ];
      const nodes: Node[] = [];
      const result = validateConnection(edges, nodes, 'C', 'A');
      expect(result.isValid).toBe(false);
      expect(result.errorMessage).toBe(
        'This connection would create a circular dependency in your workflow'
      );
    });

    it('should accept valid connection', () => {
      const edges: Edge[] = [{ id: 'e1', source: 'A', target: 'B' }];
      const nodes: Node[] = [];
      const result = validateConnection(edges, nodes, 'B', 'C');
      expect(result.isValid).toBe(true);
      expect(result.errorMessage).toBeUndefined();
    });

    it('should prioritize self-connection error over loop error', () => {
      const edges: Edge[] = [
        { id: 'e1', source: 'A', target: 'A' }, // Already has self-connection
      ];
      const nodes: Node[] = [];
      const result = validateConnection(edges, nodes, 'B', 'B');
      expect(result.isValid).toBe(false);
      expect(result.errorMessage).toBe('A step cannot connect to itself');
    });
  });

  describe('validateWorkflowStructure', () => {
    it('should require at least one step', () => {
      const nodes: Node[] = [];
      const edges: Edge[] = [];
      const result = validateWorkflowStructure(nodes, edges);
      expect(result.isValid).toBe(false);
      expect(result.errors).toContain('Workflow should have at least one step');
    });

    it('should not count note nodes as workflow steps', () => {
      const nodes: Node[] = [
        {
          id: 'note-1',
          type: 'NOTE_NODE',
          position: { x: 0, y: 0 },
          data: { content: 'A note' },
        },
      ];
      const edges: Edge[] = [];
      const result = validateWorkflowStructure(nodes, edges);
      expect(result.isValid).toBe(false);
      expect(result.errors).toContain('Workflow should have at least one step');
    });

    it('should not count create nodes as workflow steps', () => {
      const nodes: Node[] = [
        {
          id: 'create-1',
          type: 'CREATE_NODE',
          position: { x: 0, y: 0 },
          data: {},
        },
      ];
      const edges: Edge[] = [];
      const result = validateWorkflowStructure(nodes, edges);
      expect(result.isValid).toBe(false);
      expect(result.errors).toContain('Workflow should have at least one step');
    });

    it('should accept a single workflow step', () => {
      const nodes: Node[] = [
        {
          id: 'step-1',
          type: 'BASIC_NODE',
          position: { x: 0, y: 0 },
          data: { stepType: 'Action', name: 'My Step' },
        },
      ];
      const edges: Edge[] = [];
      const result = validateWorkflowStructure(nodes, edges);
      expect(result.isValid).toBe(true);
      expect(result.errors).toHaveLength(0);
    });

    it('should accept workflow with notes and steps', () => {
      const nodes: Node[] = [
        {
          id: 'note-1',
          type: 'NOTE_NODE',
          position: { x: 0, y: 0 },
          data: { content: 'A note' },
        },
        {
          id: 'step-1',
          type: 'BASIC_NODE',
          position: { x: 100, y: 0 },
          data: { stepType: 'Action', name: 'My Step' },
        },
      ];
      const edges: Edge[] = [];
      const result = validateWorkflowStructure(nodes, edges);
      expect(result.isValid).toBe(true);
      expect(result.errors).toHaveLength(0);
    });
  });
});
