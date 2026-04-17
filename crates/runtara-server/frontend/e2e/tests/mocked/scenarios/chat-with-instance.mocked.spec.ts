import { test, buildScenario } from '../../../fixtures';
import { ScenarioChatPage } from '../../../pages/ScenarioExtraPages';

test.describe('Scenario chat with instance (mocked)', () => {
  test('renders chat with running instance, a11y + snapshot', async ({
    page,
    mockApi,
    runA11y,
  }) => {
    const scenario = buildScenario({ id: 'scn_chat2', name: 'Chat scenario' });
    const instanceId = 'inst_chat';

    await mockApi.bootstrap(page);
    await mockApi.scenarios.get(page, scenario.id, scenario);
    await mockApi.scenarios.instance(page, scenario.id, instanceId, {
      data: {
        id: instanceId,
        scenarioId: scenario.id,
        status: 'RUNNING',
        created: '2026-01-01T12:00:00Z',
      },
      success: true,
    });

    const view = new ScenarioChatPage(page, scenario.id, instanceId);
    await view.goto();

    await runA11y(page, { exclude: ['[data-sonner-toaster]'] });
    await view.expectMatchesSnapshot('scenario-chat-with-instance');
  });
});
