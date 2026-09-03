import { describe, expect, it } from 'vitest';
import { render, screen } from '@testing-library/react';
import { PipelineStageRow } from './index';
import { PipelineRates } from '../PipelineRates';
import type { PipelineStage } from '../../utils/pipeline';

function stage(over: Partial<PipelineStage> & { key: string }): PipelineStage {
  return {
    label: over.key,
    knob: null,
    limit: null,
    used: null,
    oldestAgeMs: null,
    inflowKey: 'accepted',
    ...over,
  };
}

const HISTORY = Array.from({ length: 30 }, (_, i) => 8 + (i % 3));

describe('PipelineStageRow', () => {
  it('shows the knob name so an operator can act without looking it up', () => {
    render(
      <PipelineStageRow
        stage={stage({
          key: 'runPermits',
          label: 'Concurrent runs',
          knob: 'RUNTARA_MAX_CONCURRENT_RUNS',
          limit: 16,
          used: 12,
        })}
        history={HISTORY}
        inflow={398}
        pipelineActive
        isChokepoint={false}
      />
    );
    expect(screen.getByText('RUNTARA_MAX_CONCURRENT_RUNS')).toBeInTheDocument();
    expect(screen.getByText('75%')).toBeInTheDocument();
  });

  it('renders an unread occupancy as "not measured", never as 0%', () => {
    // The distinction this whole view is built around. A stage whose source
    // could not be read is unobserved, not empty, and showing 0% would present
    // a blind spot as a confident all-clear.
    render(
      <PipelineStageRow
        stage={stage({ key: 'runPermits', limit: 16, used: null })}
        history={[]}
        inflow={null}
        pipelineActive={false}
        isChokepoint={false}
      />
    );
    expect(screen.getByText('not measured')).toBeInTheDocument();
    expect(screen.queryByText('0%')).not.toBeInTheDocument();
  });

  it('marks an inflow of zero, which is where throughput died', () => {
    const { container } = render(
      <PipelineStageRow
        stage={stage({ key: 'runPermits', limit: 8, used: 8 })}
        history={HISTORY}
        inflow={0}
        pipelineActive
        isChokepoint
      />
    );
    // Reading the inflow column top to bottom, the row where it hits zero is
    // the row at fault — so zero has to be visually distinct from a small rate.
    const inflow = container.querySelector('.text-destructive');
    expect(inflow).not.toBeNull();
  });

  it('does not redden a zero inflow on an idle pipeline', () => {
    // Every stage of an idle deployment is legitimately at zero. Reddening all
    // six would cry wolf on a system doing exactly what it should, and teach
    // its reader to ignore the colour when it finally means something.
    const { container } = render(
      <PipelineStageRow
        stage={stage({ key: 'runPermits', limit: 64, used: 0 })}
        history={Array(30).fill(0)}
        inflow={0}
        pipelineActive={false}
        isChokepoint={false}
      />
    );
    expect(container.querySelector('.text-destructive')).toBeNull();
  });

  it('flags a stage that is full and not draining', () => {
    render(
      <PipelineStageRow
        stage={stage({
          key: 'runPermits',
          limit: 8,
          used: 8,
          oldestAgeMs: 2_880_000,
        })}
        history={Array(30).fill(8)}
        inflow={0}
        pipelineActive
        isChokepoint
      />
    );
    expect(screen.getByText('not draining')).toBeInTheDocument();
    expect(screen.getByText('48m oldest')).toBeInTheDocument();
  });

  it('does not flag a stage that is full but still turning work over', () => {
    // Full is not a fault. A stage pinned at its bound while recycling every
    // few seconds is a system working as hard as the host allows.
    render(
      <PipelineStageRow
        stage={stage({
          key: 'runPermits',
          limit: 16,
          used: 16,
          oldestAgeMs: 2_900,
        })}
        history={Array(30).fill(16)}
        inflow={805}
        pipelineActive
        isChokepoint={false}
      />
    );
    expect(screen.queryByText('not draining')).not.toBeInTheDocument();
    expect(screen.getByText('2.9s oldest')).toBeInTheDocument();
  });

  it('uses the server stuck policy rather than a browser-side constant', () => {
    render(
      <PipelineStageRow
        stage={stage({
          key: 'runPermits',
          limit: 16,
          used: 16,
          oldestAgeMs: 2_900,
        })}
        history={Array(30).fill(16)}
        inflow={0}
        pipelineActive
        isChokepoint
        stuckAfterMs={2_000}
      />
    );
    expect(screen.getByText('not draining')).toBeInTheDocument();
  });

  it('makes durable queue capacity retries and workflow attribution visible', () => {
    render(
      <PipelineStageRow
        stage={stage({
          key: 'launchQueued',
          label: 'Launch queue',
          limit: 64,
          used: 6,
          oldestAgeMs: 20_000,
          capacityRejections: 3,
          topWorkflows: [
            { workflowId: 'expense-approval', count: 4, oldestAgeMs: 20_000 },
            { workflowId: 'invoice-sync', count: 2, oldestAgeMs: 4_000 },
          ],
        })}
        history={Array(30).fill(6)}
        inflow={0}
        pipelineActive
        isChokepoint={false}
        stuckAfterMs={10_000}
      />
    );

    expect(screen.getByText('3 capacity retries')).toBeInTheDocument();
    expect(
      screen.getByTestId('pipeline-workflow-attribution-launchQueued')
    ).toHaveTextContent('expense-approval (4), invoice-sync (2)');
    expect(screen.getByText('not draining')).toBeInTheDocument();
  });

  it('makes a timed-out precompile child awaiting reaping visible', () => {
    render(
      <PipelineStageRow
        stage={stage({
          key: 'precompileChildren',
          label: 'Precompile children',
          limit: 2,
          used: 1,
          oldestAgeMs: 20_000,
          reapingPrecompileChildren: 1,
        })}
        history={Array(30).fill(1)}
        inflow={0}
        pipelineActive
        isChokepoint={false}
      />
    );

    expect(screen.getByText('1 child reaping')).toBeInTheDocument();
    expect(
      screen.getByTestId(
        'pipeline-reaping-precompile-children-precompileChildren'
      )
    ).toBeInTheDocument();
  });

  it('marks the chokepoint in the DOM so it can be asserted on', () => {
    const { container } = render(
      <PipelineStageRow
        stage={stage({ key: 'runPermits', limit: 8, used: 8 })}
        history={HISTORY}
        inflow={0}
        pipelineActive
        isChokepoint
      />
    );
    const row = container.querySelector(
      '[data-testid="pipeline-stage-runPermits"]'
    );
    expect(row?.getAttribute('data-chokepoint')).toBe('true');
  });

  it('shows an unbounded stage as having no limit rather than a percentage', () => {
    render(
      <PipelineStageRow
        stage={stage({ key: 'parked', label: 'Parked', used: 1_009_739 })}
        history={Array(30).fill(1_009_739)}
        inflow={396}
        pipelineActive
        isChokepoint={false}
      />
    );
    expect(screen.getByText('1.01M')).toBeInTheDocument();
    expect(screen.getByText('no limit')).toBeInTheDocument();
  });

  it('says it is still collecting rather than drawing a line from one point', () => {
    render(
      <PipelineStageRow
        stage={stage({ key: 'runPermits', limit: 16, used: 4 })}
        history={[4]}
        inflow={10}
        pipelineActive
        isChokepoint={false}
      />
    );
    expect(screen.getByText('collecting…')).toBeInTheDocument();
  });
});

describe('PipelineRates', () => {
  const RATES = {
    offered: 400,
    accepted: 400,
    denied: 0,
    started: 398,
    finished: 396,
    steps: 1980,
  };

  it('shows every headline rate', () => {
    render(<PipelineRates rates={RATES} />);
    expect(screen.getByText('Offered')).toBeInTheDocument();
    expect(screen.getByText('Denied 403')).toBeInTheDocument();
    expect(screen.getByText('Steps')).toBeInTheDocument();
    // 1,980/s renders as 2.0k: a headline tile trades the last digit for scale.
    expect(screen.getByText('2.0k')).toBeInTheDocument();
  });

  it('renders unmeasured steps as "not measured", never as 0/s', () => {
    // trackEvents is compile-time: a deployment built without it runs
    // perfectly and reports no steps. Showing 0/s beside four healthy numbers
    // would have a reader conclude work had stopped dead.
    render(<PipelineRates rates={{ ...RATES, steps: null }} />);
    expect(screen.getByText('not measured')).toBeInTheDocument();
    // And the rest of the row is unaffected — Offered and Accepted both 400.
    expect(screen.getAllByText('400')).toHaveLength(2);
  });

  it('shows dashes rather than zeros before the first window closes', () => {
    // The first tick has no earlier reading to difference against. Rendering
    // that as zeros would show a busy server as an idle one for a second.
    render(<PipelineRates rates={null} />);
    expect(screen.getAllByText('—').length).toBeGreaterThan(0);
  });
});
