import type { Preview } from '@storybook/react';
import { withThemeByClassName } from '@storybook/addon-themes';

// Import global styles including Tailwind
import '../src/index.css';

const preview: Preview = {
  parameters: {
    controls: {
      matchers: {
        color: /(background|color)$/i,
        date: /Date$/i,
      },
    },
    backgrounds: {
      disable: true, // Use theme addon instead
    },
    layout: 'centered',
    docs: {
      toc: true,
    },
  },
  decorators: [
    // Theme decorator - handles dark mode via class strategy
    withThemeByClassName({
      themes: {
        light: '',
        dark: 'dark',
      },
      defaultTheme: 'light',
    }),
  ],
  tags: ['autodocs'],
};

export default preview;
