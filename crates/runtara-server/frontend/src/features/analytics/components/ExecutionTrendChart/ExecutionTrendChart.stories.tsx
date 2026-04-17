import type { Meta, StoryObj } from '@storybook/react';
import { ExecutionTrendChart } from './index';

const meta: Meta<typeof ExecutionTrendChart> = {
  title: 'Analytics/ExecutionTrendChart',
  component: ExecutionTrendChart,
  parameters: {
    layout: 'padded',
    docs: {
      description: {
        component:
          'A line chart component for displaying execution trends over time. Shows executions, success rate, and optionally memory usage. Uses Recharts for rendering.',
      },
    },
  },
  tags: ['autodocs'],
  argTypes: {
    loading: {
      control: 'boolean',
      description: 'Show loading skeleton',
    },
  },
};

export default meta;
type Story = StoryObj<typeof ExecutionTrendChart>;

// Helper to generate sample data
const generateTrendData = (
  hours: number,
  baseExecutions: number = 100,
  includeMemory: boolean = false
) => {
  const data = [];
  const now = new Date();

  for (let i = hours; i >= 0; i--) {
    const timestamp = new Date(now.getTime() - i * 60 * 60 * 1000);
    const variance = Math.random() * 0.4 - 0.2; // ±20% variance
    const executions = Math.round(baseExecutions * (1 + variance));
    const successRate = 95 + Math.random() * 5; // 95-100%

    data.push({
      timestamp: timestamp.toISOString(),
      executions,
      successRate: Math.round(successRate * 10) / 10,
      avgDuration: Math.round((Math.random() * 2 + 0.5) * 100) / 100,
      ...(includeMemory && {
        avgMemory: Math.round((Math.random() * 200 + 100) * 10) / 10,
      }),
    });
  }

  return data;
};

// Generate data for different time ranges
const hourlyData = generateTrendData(1, 50);
const dailyData = generateTrendData(24, 100);
const weeklyData = generateTrendData(24 * 7, 150);
const monthlyData = generateTrendData(24 * 30, 200);

// Data with memory
const dataWithMemory = generateTrendData(24, 100, true);

// Degraded performance data
const degradedData = Array.from({ length: 24 }, (_, i) => {
  const timestamp = new Date(Date.now() - (24 - i) * 60 * 60 * 1000);
  // Simulate degraded performance in the middle
  const isDegraded = i >= 10 && i <= 16;
  return {
    timestamp: timestamp.toISOString(),
    executions: isDegraded
      ? Math.round(50 + Math.random() * 30)
      : Math.round(100 + Math.random() * 50),
    successRate: isDegraded ? 75 + Math.random() * 15 : 95 + Math.random() * 5,
    avgDuration: isDegraded ? 3 + Math.random() * 2 : 1 + Math.random() * 0.5,
  };
});

export const Default: Story = {
  args: {
    data: dailyData,
  },
};

export const Loading: Story = {
  args: {
    data: [],
    loading: true,
  },
};

export const Empty: Story = {
  name: 'Empty State',
  args: {
    data: [],
    loading: false,
  },
};

export const HourlyData: Story = {
  name: 'Hourly Data (1 Hour)',
  args: {
    data: hourlyData,
  },
  parameters: {
    docs: {
      description: {
        story: 'Shows time format HH:mm:ss for short time spans.',
      },
    },
  },
};

export const DailyData: Story = {
  name: 'Daily Data (24 Hours)',
  args: {
    data: dailyData,
  },
  parameters: {
    docs: {
      description: {
        story: 'Shows time format HH:mm for within-day spans.',
      },
    },
  },
};

export const WeeklyData: Story = {
  name: 'Weekly Data (7 Days)',
  args: {
    data: weeklyData,
  },
  parameters: {
    docs: {
      description: {
        story: 'Shows date format MMM dd HH:mm for weekly spans.',
      },
    },
  },
};

export const MonthlyData: Story = {
  name: 'Monthly Data (30 Days)',
  args: {
    data: monthlyData,
  },
  parameters: {
    docs: {
      description: {
        story: 'Shows date format MMM dd for longer spans.',
      },
    },
  },
};

export const WithMemoryUsage: Story = {
  name: 'With Memory Usage',
  args: {
    data: dataWithMemory,
  },
  parameters: {
    docs: {
      description: {
        story:
          'Shows an additional memory usage chart when memory data is present.',
      },
    },
  },
};

export const DegradedPerformance: Story = {
  name: 'Degraded Performance',
  args: {
    data: degradedData,
  },
  parameters: {
    docs: {
      description: {
        story:
          'Shows a period of degraded performance with lower success rates.',
      },
    },
  },
};

export const ExecutionsOnly: Story = {
  name: 'Executions Only',
  args: {
    data: dailyData.map((d) => ({
      timestamp: d.timestamp,
      executions: d.executions,
    })),
  },
  parameters: {
    docs: {
      description: {
        story: 'Chart with only execution counts, no success rate line.',
      },
    },
  },
};

export const HighVolume: Story = {
  name: 'High Volume',
  args: {
    data: generateTrendData(24, 10000),
  },
  parameters: {
    docs: {
      description: {
        story: 'Shows how the chart handles high volume execution data.',
      },
    },
  },
};

export const LowVolume: Story = {
  name: 'Low Volume',
  args: {
    data: generateTrendData(24, 5),
  },
  parameters: {
    docs: {
      description: {
        story: 'Shows how the chart handles low volume execution data.',
      },
    },
  },
};
