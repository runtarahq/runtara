import { test, buildScenario } from '../../../fixtures';
import { ScenarioLogsPage } from '../../../pages/ScenarioExtraPages';

test.describe('Scenario execution logs (mocked)', () => {
  test('renders logs view, a11y + snapshot', async ({
    page,
    mockApi,
    runA11y,
  }) => {
    const scenario = buildScenario({ id: 'scn_logs', name: 'Logs scenario' });
    const instanceId = 'inst_logs';

    await mockApi.bootstrap(page);
    await mockApi.scenarios.get(page, scenario.id, scenario);
    await mockApi.scenarios.instance(page, scenario.id, instanceId, {
      data: {
        id: instanceId,
        scenarioId: scenario.id,
        status: 'COMPLETED',
        created: '2026-01-01T12:00:00Z',
        finished: '2026-01-01T12:00:10Z',
      },
      success: true,
    });
    await mockApi.scenarios.logs(page, scenario.id, instanceId, {
      logs: [
        {
          timestamp: '2026-01-01T12:00:01Z',
          level: 'INFO',
          message: 'Scenario started',
        },
        {
          timestamp: '2026-01-01T12:00:10Z',
          level: 'INFO',
          message: 'Scenario completed',
        },
      ],
    });

    const view = new ScenarioLogsPage(page, scenario.id, instanceId);
    await view.goto();

    await runA11y(page, { exclude: ['[data-sonner-toaster]'] });
    await view.expectMatchesSnapshot('scenario-logs');
  });
});
