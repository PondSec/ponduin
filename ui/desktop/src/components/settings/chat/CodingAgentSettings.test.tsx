import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { IntlTestWrapper } from '../../../i18n/test-utils';
import { CodingAgentSettings } from './CodingAgentSettings';

const configMock = vi.hoisted(() => ({
  values: {} as Record<string, unknown>,
  upsert: vi.fn(),
}));

vi.mock('../../ConfigContext', () => ({
  useConfig: () => ({
    config: configMock.values,
    upsert: configMock.upsert,
  }),
}));

const renderSettings = () =>
  render(<CodingAgentSettings />, {
    wrapper: IntlTestWrapper,
  });

describe('CodingAgentSettings', () => {
  beforeEach(() => {
    configMock.values = {};
    configMock.upsert.mockReset();
    configMock.upsert.mockResolvedValue(undefined);
  });

  it('states that only Autonomous removes coding confirmations', () => {
    configMock.values = {
      PONDUIN_CODING_ENABLED: true,
      PONDUIN_CODING_MODE: 'debugging',
      PONDUIN_MODE: 'approve',
    };

    renderSettings();

    expect(
      screen.getByText(
        'Only Autonomous mode runs coding tools without confirmation. Manual and Smart retain approval gates; Chat disables all tools.'
      )
    ).toBeInTheDocument();
    expect(screen.getByLabelText('Coding task mode')).toHaveValue('debugging');
  });

  it('shows the hard-security boundary in Autonomous mode', () => {
    configMock.values = {
      PONDUIN_CODING_ENABLED: true,
      PONDUIN_CODING_MODE: 'coding',
      PONDUIN_MODE: 'auto',
    };

    renderSettings();

    expect(
      screen.getByText(
        'Autonomous mode is active: coding tools can run without confirmation. Hard security blocks still apply.'
      )
    ).toBeInTheDocument();
  });

  it('enables a usable coding mode when the previous mode was general', async () => {
    const user = userEvent.setup();
    configMock.values = {
      PONDUIN_CODING_ENABLED: false,
      PONDUIN_CODING_MODE: 'general',
      PONDUIN_MODE: 'auto',
    };

    renderSettings();
    await user.click(screen.getByRole('switch', { name: 'Enable internal coding agent' }));

    await waitFor(() => {
      expect(configMock.upsert).toHaveBeenNthCalledWith(1, 'PONDUIN_CODING_MODE', 'coding', false);
      expect(configMock.upsert).toHaveBeenNthCalledWith(2, 'PONDUIN_CODING_ENABLED', true, false);
    });
  });
});
