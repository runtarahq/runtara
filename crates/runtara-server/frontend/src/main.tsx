import { loadStaticConfig } from '@/shared/config/staticConfig';

// Boot in two stages: a static host's config.json has to be in place before
// any module reads the runtime config, and most of the app reads it when it is
// first imported. So the app is imported only once the config is settled.
void loadStaticConfig().then((boot) => {
  if (boot) void import('./bootstrap');
});
