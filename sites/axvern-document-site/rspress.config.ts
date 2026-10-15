import * as path from 'node:path';
import { defineConfig } from '@rspress/core';

export default defineConfig({
  title: 'axvern',
  description: 'Policy-aware asynchronous command execution with Pueue.',
  lang: 'en',
  // One maintained language; no untranslated template pages.
  root: path.join(__dirname, 'docs/en'),
  themeConfig: {
    socialLinks: [
      {
        icon: 'github',
        mode: 'link',
        content: 'https://github.com/UtakataKyosui/axvern',
      },
    ],
  },
});
