import React from 'react';
import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { IntlTestWrapper } from '../../i18n/test-utils';
import { acpDeleteSession, acpListSessions } from '../../acp/sessions';
import SessionListView from './SessionListView';

vi.mock('../../acp/sessions', () => ({
  acpDeleteSession: vi.fn(),
  acpExportSession: vi.fn(),
  acpForkSession: vi.fn(),
  acpImportSession: vi.fn(),
  acpListSessions: vi.fn(),
  acpRenameSession: vi.fn(),
  acpShareSessionNostr: vi.fn(),
}));

vi.mock('../../acp/chatSessionStore', () => ({
  acpChatSessionActions: { deleteSnapshot: vi.fn() },
}));

vi.mock('../../acp/permissionRequests', () => ({
  cancelAcpPermissionRequestsForSession: vi.fn(),
}));

vi.mock('../../acp/elicitationRequests', () => ({
  cancelAcpElicitationRequestsForSession: vi.fn(),
}));

vi.mock('../Layout/MainPanelLayout', () => ({
  MainPanelLayout: ({ children }: { children: React.ReactNode }) => <main>{children}</main>,
}));

vi.mock('../conversation/SearchView', () => ({
  SearchView: ({ children }: { children: React.ReactNode }) => <>{children}</>,
}));

vi.mock('../ui/scroll-area', () => ({
  ScrollArea: ({ children }: { children: React.ReactNode }) => <>{children}</>,
}));

vi.mock('../ui/ConfirmationModal', () => ({
  ConfirmationModal: ({
    isOpen,
    title,
    message,
    confirmLabel,
    onConfirm,
    onCancel,
  }: {
    isOpen: boolean;
    title: string;
    message: string;
    confirmLabel: string;
    onConfirm: () => void;
    onCancel: () => void;
  }) =>
    isOpen ? (
      <section aria-label="Delete confirmation">
        <h2>{title}</h2>
        <p>{message}</p>
        <button onClick={onConfirm}>{confirmLabel}</button>
        <button onClick={onCancel}>Cancel</button>
      </section>
    ) : null,
}));

vi.mock('react-toastify', () => ({
  toast: { error: vi.fn(), success: vi.fn() },
}));

const sessions = [
  {
    id: 'session-1',
    name: 'First session',
    workingDir: '/work/first',
    updatedAt: '2026-08-24T10:00:00Z',
    createdAt: '2026-08-24T09:00:00Z',
    messageCount: 3,
  },
  {
    id: 'session-2',
    name: 'Second session',
    workingDir: '/work/second',
    updatedAt: '2026-08-24T09:00:00Z',
    createdAt: '2026-08-24T08:00:00Z',
    messageCount: 5,
  },
];

describe('SessionListView', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    Object.assign(window.electron, { getConfig: vi.fn(() => ({})) });
    vi.mocked(acpListSessions)
      .mockResolvedValueOnce({ sessions, nextCursor: null })
      .mockResolvedValue({ sessions: [], nextCursor: null });
    vi.mocked(acpDeleteSession).mockResolvedValue(undefined);
  });

  it('deletes multiple selected sessions after one confirmation', async () => {
    const user = userEvent.setup();
    render(<SessionListView onSelectSession={vi.fn()} />, { wrapper: IntlTestWrapper });

    await screen.findByText('First session');
    await user.click(screen.getByRole('button', { name: 'Select sessions' }));
    await user.click(screen.getByRole('button', { name: 'Select session "First session"' }));
    await user.click(screen.getByRole('button', { name: 'Select session "Second session"' }));

    expect(screen.getByText('2 selected')).toBeInTheDocument();

    await user.click(screen.getByRole('button', { name: 'Delete selected' }));
    expect(screen.getByRole('heading', { name: 'Delete 2 Sessions' })).toBeInTheDocument();

    await user.click(screen.getByRole('button', { name: 'Delete 2 Sessions' }));

    await waitFor(() => {
      expect(acpDeleteSession).toHaveBeenCalledTimes(2);
    });
    expect(acpDeleteSession).toHaveBeenCalledWith('session-1');
    expect(acpDeleteSession).toHaveBeenCalledWith('session-2');
  });
});
