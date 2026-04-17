import { test, buildScenario } from '../../../fixtures';
import { ScenarioChatPage } from '../../../pages/ScenarioExtraPages';

test.describe('Scenario chat without instance (mocked)', () => {
  test('renders chat shell, a11y + snapshot', async ({
    page,
    mockApi,
    runA11y,
  }) => {
    const scenario = buildScenario({ id: 'scn_chat', name: 'Chat scenario' });

    await mockApi.bootstrap(page);
    await mockApi.scenarios.get(page, scenario.id, scenario);

    const view = new ScenarioChatPage(page, scenario.id);
    await view.goto();

    await runA11y(page, { exclude: ['[data-sonner-toaster]'] });
    await view.expectMatchesSnapshot('scenario-chat-empty');
  });
});
