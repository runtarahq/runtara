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
          label: 'Run permits',
          knob: 'RUNTARA_MAX_CONCURRENT_RUNS',
          limit: 16,
          used: 12,
        })}
        history={HISTORY}
        inflow={398}
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
        isChokepoint
      />
    );
    // Reading the inflow column top to bottom, the row where it hits zero is
    // the row at fault — so zero has to be visually distinct from a small rate.
    const inflow = container.querySelector('.text-destructive');
    expect(inflow).not.toBeNull();
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
        isChokepoint={false}
      />
    );
    expect(screen.queryByText('not draining')).not.toBeInTheDocument();
    expect(screen.getByText('2.9s oldest')).toBeInTheDocument();
  });

  it('marks the chokepoint in the DOM so it can be asserted on', () => {
    const { container } = render(
      <PipelineStageRow
        stage={stage({ key: 'runPermits', limit: 8, used: 8 })}
        history={HISTORY}
        inflow={0}
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
