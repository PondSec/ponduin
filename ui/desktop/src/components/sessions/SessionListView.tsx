import { AppEvents } from '../../constants/events';
import React, { useEffect, useState, useRef, useCallback, useMemo, startTransition } from 'react';
import { defineMessages, useIntl } from '../../i18n';
import {
  MessageSquareText,
  AlertCircle,
  Calendar,
  Folder,
  Edit2,
  Trash2,
  Download,
  Upload,
  Share2,
  LoaderCircle,
  ExternalLink,
  Copy,
  CheckSquare,
  Square,
} from 'lucide-react';
import { Card } from '../ui/card';
import { Button } from '../ui/button';
import { ScrollArea } from '../ui/scroll-area';
import { formatMessageTimestamp } from '../../utils/timeUtils';
import { SearchView } from '../conversation/SearchView';
import { MainPanelLayout } from '../Layout/MainPanelLayout';
import { groupSessionsByDate, sessionActivityAt, type DateGroup } from '../../utils/dateUtils';
import { errorMessage } from '../../utils/conversionUtils';
import { Skeleton } from '../ui/skeleton';
import { toast } from 'react-toastify';
import { ConfirmationModal } from '../ui/ConfirmationModal';
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '../ui/dialog';
import {
  acpDeleteSession,
  acpExportSession,
  acpForkSession,
  acpImportSession,
  acpListSessions,
  acpRenameSession,
  acpShareSessionNostr,
  type SessionListItem,
} from '../../acp/sessions';
import { acpChatSessionActions } from '../../acp/chatSessionStore';
import { cancelAcpPermissionRequestsForSession } from '../../acp/permissionRequests';
import { cancelAcpElicitationRequestsForSession } from '../../acp/elicitationRequests';
import { getSearchShortcutText } from '../../utils/keyboardShortcuts';

const i18n = defineMessages({
  editSessionTitle: { id: 'sessions.edit.title', defaultMessage: 'Edit Session Description' },
  editSessionPlaceholder: {
    id: 'sessions.edit.placeholder',
    defaultMessage: 'Enter session description',
  },
  cancel: { id: 'sessions.cancel', defaultMessage: 'Cancel' },
  save: { id: 'sessions.save', defaultMessage: 'Save' },
  saving: { id: 'sessions.saving', defaultMessage: 'Saving...' },
  sessionUpdated: {
    id: 'sessions.toast.updated',
    defaultMessage: 'Session description updated successfully',
  },
  sessionUpdateFailed: {
    id: 'sessions.toast.updateFailed',
    defaultMessage: 'Failed to update session description: {error}',
  },
  chatHistory: { id: 'sessions.chatHistory', defaultMessage: 'Chat history' },
  importSession: { id: 'sessions.import', defaultMessage: 'Import Session' },
  importNostrSession: { id: 'sessions.importNostr', defaultMessage: 'Import Link' },
  importNostrTitle: { id: 'sessions.importNostr.title', defaultMessage: 'Import Nostr Session' },
  importNostrDesc: {
    id: 'sessions.importNostr.description',
    defaultMessage: 'Paste a Ponduin Nostr share link to fetch, decrypt, and import the session.',
  },
  importNostrPlaceholder: {
    id: 'sessions.importNostr.placeholder',
    defaultMessage: 'ponduin://sessions/nostr?nevent=...&key=...',
  },
  importing: { id: 'sessions.importing', defaultMessage: 'Importing...' },
  chatHistoryDesc: {
    id: 'sessions.chatHistoryDesc',
    defaultMessage: 'View and search your past conversations with Ponduin. {shortcut} to search.',
  },
  searchPlaceholder: { id: 'sessions.searchPlaceholder', defaultMessage: 'Search history...' },
  errorLoading: { id: 'sessions.error.loading', defaultMessage: 'Error Loading Sessions' },
  tryAgain: { id: 'sessions.error.tryAgain', defaultMessage: 'Try Again' },
  noSessions: { id: 'sessions.empty.title', defaultMessage: 'No chat sessions found' },
  noSessionsDesc: {
    id: 'sessions.empty.description',
    defaultMessage: 'Your chat history will appear here',
  },
  noMatching: { id: 'sessions.search.noResults', defaultMessage: 'No matching sessions found' },
  noMatchingDesc: {
    id: 'sessions.search.noResultsDesc',
    defaultMessage: 'Try adjusting your search terms',
  },
  loadingMore: { id: 'sessions.loadingMore', defaultMessage: 'Loading more sessions...' },
  deleteTitle: { id: 'sessions.delete.title', defaultMessage: 'Delete Session' },
  deleteMessage: {
    id: 'sessions.delete.message',
    defaultMessage:
      'Are you sure you want to delete the session "{name}"? This action cannot be undone.',
  },
  deleteManyTitle: { id: 'sessions.deleteMany.title', defaultMessage: 'Delete {count} Sessions' },
  deleteManyMessage: {
    id: 'sessions.deleteMany.message',
    defaultMessage:
      'Are you sure you want to delete {count} sessions? This action cannot be undone.',
  },
  duplicateSuccess: {
    id: 'sessions.toast.duplicated',
    defaultMessage: 'Session "{name}" duplicated successfully',
  },
  duplicateFailed: {
    id: 'sessions.toast.duplicateFailed',
    defaultMessage: 'Failed to duplicate session: {error}',
  },
  deleteSuccess: { id: 'sessions.toast.deleted', defaultMessage: 'Session deleted successfully' },
  deleteFailed: {
    id: 'sessions.toast.deleteFailed',
    defaultMessage: 'Failed to delete session "{name}": {error}',
  },
  deleteManySuccess: {
    id: 'sessions.toast.deleteMany',
    defaultMessage: '{count} sessions deleted successfully',
  },
  deleteManyFailed: {
    id: 'sessions.toast.deleteManyFailed',
    defaultMessage: 'Failed to delete {count} sessions. Please try again.',
  },
  importSuccess: { id: 'sessions.toast.imported', defaultMessage: 'Session imported successfully' },
  importFailed: {
    id: 'sessions.toast.importFailed',
    defaultMessage: 'Failed to import session: {error}',
  },
  exportSuccess: { id: 'sessions.toast.exported', defaultMessage: 'Session exported successfully' },
  shareNostrSuccess: {
    id: 'sessions.toast.shareNostr',
    defaultMessage: 'Encrypted Nostr share link created',
  },
  shareNostrFailed: {
    id: 'sessions.toast.shareNostrFailed',
    defaultMessage: 'Failed to create Nostr share link: {error}',
  },
  copied: { id: 'sessions.toast.copied', defaultMessage: 'Copied to clipboard' },
  openInNewWindow: { id: 'sessions.action.openNewWindow', defaultMessage: 'Open in new window' },
  editSessionName: { id: 'sessions.action.editName', defaultMessage: 'Edit session name' },
  duplicateSession: { id: 'sessions.action.duplicate', defaultMessage: 'Duplicate session' },
  deleteSession: { id: 'sessions.action.delete', defaultMessage: 'Delete session' },
  selectSessions: { id: 'sessions.action.select', defaultMessage: 'Select sessions' },
  cancelSelection: { id: 'sessions.action.cancelSelection', defaultMessage: 'Cancel selection' },
  selectAll: { id: 'sessions.action.selectAll', defaultMessage: 'Select all visible' },
  clearSelection: { id: 'sessions.action.clearSelection', defaultMessage: 'Clear selection' },
  deleteSelected: { id: 'sessions.action.deleteSelected', defaultMessage: 'Delete selected' },
  selectedCount: { id: 'sessions.selection.count', defaultMessage: '{count} selected' },
  selectSession: { id: 'sessions.selection.select', defaultMessage: 'Select session "{name}"' },
  deselectSession: {
    id: 'sessions.selection.deselect',
    defaultMessage: 'Deselect session "{name}"',
  },
  exportSession: { id: 'sessions.action.export', defaultMessage: 'Export session' },
  shareNostrSession: {
    id: 'sessions.action.shareNostr',
    defaultMessage: 'Share encrypted Nostr link',
  },
  shareNostrTitle: {
    id: 'sessions.shareNostr.title',
    defaultMessage: 'Encrypted Nostr Share Link',
  },
  shareNostrDesc: {
    id: 'sessions.shareNostr.description',
    defaultMessage:
      'Anyone with this link can fetch and decrypt the session. Treat it like a secret.',
  },
  close: { id: 'sessions.close', defaultMessage: 'Close' },
});

interface EditSessionModalProps {
  session: SessionListItem | null;
  isOpen: boolean;
  onClose: () => void;
  onSave: (sessionId: string, newDescription: string) => Promise<void>;
  disabled?: boolean;
}

const EditSessionModal = React.memo<EditSessionModalProps>(
  ({ session, isOpen, onClose, onSave, disabled = false }) => {
    const intl = useIntl();
    const [description, setDescription] = useState('');
    const [isUpdating, setIsUpdating] = useState(false);

    useEffect(() => {
      if (session && isOpen) {
        setDescription(session.name);
      } else if (!isOpen) {
        setDescription('');
        setIsUpdating(false);
      }
    }, [session, isOpen]);

    const handleSave = useCallback(async () => {
      if (!session || disabled) return;

      const trimmedDescription = description.trim();
      if (trimmedDescription === session.name) {
        onClose();
        return;
      }

      setIsUpdating(true);
      try {
        await acpRenameSession(session.id, trimmedDescription);
        await onSave(session.id, trimmedDescription);
        onClose();
        setTimeout(() => {
          toast.success(intl.formatMessage(i18n.sessionUpdated));
        }, 300);
      } catch (error) {
        const errMsg = errorMessage(error, 'Unknown error occurred');
        console.error('Failed to update session description:', errMsg);
        toast.error(intl.formatMessage(i18n.sessionUpdateFailed, { error: errMsg }));
        setDescription(session.name);
      } finally {
        setIsUpdating(false);
      }
    }, [session, description, onSave, onClose, disabled, intl]);

    const handleCancel = useCallback(() => {
      if (!isUpdating) {
        onClose();
      }
    }, [onClose, isUpdating]);

    const handleKeyDown = useCallback(
      (e: React.KeyboardEvent<HTMLInputElement>) => {
        if (e.key === 'Enter' && !isUpdating) {
          handleSave();
        } else if (e.key === 'Escape' && !isUpdating) {
          handleCancel();
        }
      },
      [handleSave, handleCancel, isUpdating]
    );

    const handleInputChange = useCallback((e: React.ChangeEvent<HTMLInputElement>) => {
      setDescription(e.target.value);
    }, []);

    if (!isOpen || !session) return null;

    return (
      <div className="fixed inset-0 z-[300] flex items-center justify-center bg-black/50">
        <div className="bg-background-primary border border-border-primary rounded-lg p-6 w-[500px] max-w-[90vw]">
          <h3 className="text-lg font-medium text-text-primary mb-4">
            {intl.formatMessage(i18n.editSessionTitle)}
          </h3>

          <div className="space-y-4">
            <div>
              <input
                id="session-description"
                type="text"
                value={description}
                onChange={handleInputChange}
                className="w-full p-3 border border-border-primary rounded-lg bg-background-primary text-text-primary focus:outline-none focus:ring-2 focus:ring-blue-500"
                placeholder={intl.formatMessage(i18n.editSessionPlaceholder)}
                autoFocus
                maxLength={200}
                onKeyDown={handleKeyDown}
                disabled={isUpdating || disabled}
              />
            </div>
          </div>

          <div className="flex justify-end space-x-3 mt-6">
            <Button onClick={handleCancel} variant="ghost" disabled={isUpdating || disabled}>
              {intl.formatMessage(i18n.cancel)}
            </Button>
            <Button
              onClick={handleSave}
              disabled={!description.trim() || isUpdating || disabled}
              variant="default"
            >
              {isUpdating ? intl.formatMessage(i18n.saving) : intl.formatMessage(i18n.save)}
            </Button>
          </div>
        </div>
      </div>
    );
  }
);

EditSessionModal.displayName = 'EditSessionModal';

// Debounce hook for search
function useDebounce<T>(value: T, delay: number): T {
  const [debouncedValue, setDebouncedValue] = useState<T>(value);

  useEffect(() => {
    const handler = setTimeout(() => {
      setDebouncedValue(value);
    }, delay);

    return () => {
      window.clearTimeout(handler);
    };
  }, [value, delay]);

  return debouncedValue;
}

interface SessionListViewProps {
  onSelectSession: (sessionId: string) => void;
}

const SessionListView: React.FC<SessionListViewProps> = React.memo(({ onSelectSession }) => {
  const intl = useIntl();
  const [sessions, setSessions] = useState<SessionListItem[]>([]);
  const [isPrefetchingSessions, setIsPrefetchingSessions] = useState(false);
  const [dateGroups, setDateGroups] = useState<DateGroup[]>([]);
  const [isLoading, setIsLoading] = useState(true);
  const [showSkeleton, setShowSkeleton] = useState(true);
  const [showContent, setShowContent] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const [visibleGroupsCount, setVisibleGroupsCount] = useState(15);

  // Edit modal state
  const [showEditModal, setShowEditModal] = useState(false);
  const [editingSession, setEditingSession] = useState<SessionListItem | null>(null);

  // Delete confirmation modal state
  const [showDeleteConfirmation, setShowDeleteConfirmation] = useState(false);
  const [sessionsToDelete, setSessionsToDelete] = useState<SessionListItem[]>([]);
  const [isDeletingSessions, setIsDeletingSessions] = useState(false);
  const [isSelectionMode, setIsSelectionMode] = useState(false);
  const [selectedSessionIds, setSelectedSessionIds] = useState<Set<string>>(() => new Set());

  const [showImportLinkModal, setShowImportLinkModal] = useState(false);
  const [nostrImportLink, setNostrImportLink] = useState('');
  const [isImportingNostr, setIsImportingNostr] = useState(false);
  const [shareLink, setShareLink] = useState('');
  const [showShareLinkModal, setShowShareLinkModal] = useState(false);
  const [sharingSessionId, setSharingSessionId] = useState<string | null>(null);
  const [nostrEnabled, setNostrEnabled] = useState(true);

  // Search state for debouncing
  const [searchTerm, setSearchTerm] = useState('');
  const debouncedSearchTerm = useDebounce(searchTerm, 300); // 300ms debounce
  const debouncedSearchTermRef = useRef(debouncedSearchTerm);
  debouncedSearchTermRef.current = debouncedSearchTerm;

  const containerRef = useRef<HTMLDivElement>(null);
  const loadGenerationRef = useRef(0);
  const hasLoadedRef = useRef(false);

  const fileInputRef = useRef<HTMLInputElement>(null);

  const visibleDateGroups = useMemo(() => {
    return dateGroups.slice(0, visibleGroupsCount);
  }, [dateGroups, visibleGroupsCount]);

  const previousSearchTermRef = useRef('');
  useEffect(() => {
    const wasSearching = previousSearchTermRef.current.length > 0;
    const isSearching = debouncedSearchTerm.length > 0;
    previousSearchTermRef.current = debouncedSearchTerm;

    if (isSearching) {
      setVisibleGroupsCount(dateGroups.length);
    } else if (wasSearching) {
      setVisibleGroupsCount(15);
    }
  }, [debouncedSearchTerm, dateGroups.length]);

  const allVisibleSessionsSelected =
    sessions.length > 0 && sessions.every((session) => selectedSessionIds.has(session.id));

  const loadRemainingSessionPages = useCallback(
    async (initialCursor: string, loadId: number, keyword?: string) => {
      let cursor: string | null = initialCursor;
      setIsPrefetchingSessions(true);

      try {
        while (cursor && loadGenerationRef.current === loadId) {
          const resp = await acpListSessions(cursor, { keyword });
          if (loadGenerationRef.current !== loadId) return;

          cursor = resp.nextCursor;
          startTransition(() => {
            setSessions((prev) => {
              const seen = new Set(prev.map((s) => s.id));
              return [...prev, ...resp.sessions.filter((s) => !seen.has(s.id))];
            });
          });
        }
      } catch (err) {
        console.error('Failed to load remaining sessions:', err);
      } finally {
        if (loadGenerationRef.current === loadId) {
          setIsPrefetchingSessions(false);
        }
      }
    },
    []
  );

  const loadSessions = useCallback(
    async (keyword: string = debouncedSearchTermRef.current) => {
      const loadId = loadGenerationRef.current + 1;
      loadGenerationRef.current = loadId;
      // Only show the skeleton on the first load; subsequent loads (e.g. typing a
      // search keyword) update the list in place without flashing the skeleton.
      const isFirstLoad = !hasLoadedRef.current;
      setIsPrefetchingSessions(false);
      setError(null);
      if (isFirstLoad) {
        setIsLoading(true);
        setShowSkeleton(true);
        setShowContent(false);
      }
      try {
        const resp = await acpListSessions(undefined, { keyword });
        if (loadGenerationRef.current !== loadId) return;
        hasLoadedRef.current = true;

        startTransition(() => {
          setSessions(resp.sessions);
        });

        if (resp.nextCursor) {
          void loadRemainingSessionPages(resp.nextCursor, loadId, keyword);
        }
      } catch (err) {
        if (loadGenerationRef.current !== loadId) return;

        console.error('Failed to load sessions:', err);
        setError('Failed to load sessions. Please try again later.');
        setSessions([]);
      } finally {
        if (loadGenerationRef.current === loadId && isFirstLoad) {
          setIsLoading(false);
        }
      }
    },
    [loadRemainingSessionPages]
  );

  const handleScroll = useCallback(
    (target: HTMLDivElement) => {
      const { scrollTop, scrollHeight, clientHeight } = target;
      const threshold = 200;

      if (scrollHeight - scrollTop - clientHeight >= threshold) return;

      if (visibleGroupsCount < dateGroups.length) {
        setVisibleGroupsCount((prev) => Math.min(prev + 5, dateGroups.length));
      }
    },
    [visibleGroupsCount, dateGroups.length]
  );

  useEffect(() => {
    loadSessions(debouncedSearchTerm);
    return () => {
      // Bump the generation so any in-flight load for the previous keyword is discarded.
      loadGenerationRef.current += 1;
    };
  }, [loadSessions, debouncedSearchTerm]);

  // Hide Nostr sharing when explicitly disabled via env var (restricted/enterprise bundles)
  useEffect(() => {
    const config = window.electron.getConfig();
    if (config.PONDUIN_DISABLE_NOSTR_SHARING === true) {
      setNostrEnabled(false);
    }
  }, []);

  // Timing logic to prevent flicker between skeleton and content on initial load
  useEffect(() => {
    if (!isLoading && showSkeleton) {
      setShowSkeleton(false);
      // Use startTransition for non-blocking content show
      startTransition(() => {
        setTimeout(() => {
          setShowContent(true);
        }, 10);
      });
    }
    return () => void 0;
  }, [isLoading, showSkeleton]);

  // Memoize date groups calculation to prevent unnecessary recalculations
  const memoizedDateGroups = useMemo(() => {
    if (sessions.length > 0) {
      return groupSessionsByDate(sessions);
    }
    return [];
  }, [sessions]);

  // Update date groups when filtered sessions change
  useEffect(() => {
    startTransition(() => {
      setDateGroups(memoizedDateGroups);
    });
  }, [memoizedDateGroups]);

  useEffect(() => {
    const availableSessionIds = new Set(sessions.map((session) => session.id));
    setSelectedSessionIds((previous) => {
      const next = new Set([...previous].filter((sessionId) => availableSessionIds.has(sessionId)));
      return next.size === previous.size ? previous : next;
    });
  }, [sessions]);

  // Handle immediate search input (updates search term for debouncing).
  const handleSearch = useCallback((term: string) => {
    setSearchTerm(term);
  }, []);

  // Handle modal close
  const handleModalClose = useCallback(() => {
    setShowEditModal(false);
    setEditingSession(null);
  }, []);

  const handleModalSave = useCallback(async (sessionId: string, newDescription: string) => {
    // Update state immediately for optimistic UI
    setSessions((prevSessions) =>
      prevSessions.map((s) =>
        s.id === sessionId ? { ...s, name: newDescription, user_set_name: true } : s
      )
    );
    window.dispatchEvent(
      new CustomEvent(AppEvents.SESSION_RENAMED, {
        detail: { sessionId, newName: newDescription, userInitiated: true },
      })
    );
  }, []);

  const handleEditSession = useCallback((session: SessionListItem) => {
    setEditingSession(session);
    setShowEditModal(true);
  }, []);

  const handleDeleteSession = useCallback((session: SessionListItem) => {
    setSessionsToDelete([session]);
    setShowDeleteConfirmation(true);
  }, []);

  const handleSelectionModeChange = useCallback((enabled: boolean) => {
    setIsSelectionMode(enabled);
    if (!enabled) {
      setSelectedSessionIds(new Set());
    }
  }, []);

  const handleToggleSessionSelection = useCallback((sessionId: string) => {
    setSelectedSessionIds((previous) => {
      const next = new Set(previous);
      if (next.has(sessionId)) {
        next.delete(sessionId);
      } else {
        next.add(sessionId);
      }
      return next;
    });
  }, []);

  const handleSelectAllVisibleSessions = useCallback(() => {
    setSelectedSessionIds((previous) => {
      if (allVisibleSessionsSelected) {
        return new Set();
      }
      return new Set([...previous, ...sessions.map((session) => session.id)]);
    });
  }, [allVisibleSessionsSelected, sessions]);

  const handleDeleteSelectedSessions = useCallback(() => {
    const selectedSessions = sessions.filter((session) => selectedSessionIds.has(session.id));
    if (selectedSessions.length === 0) return;

    setSessionsToDelete(selectedSessions);
    setShowDeleteConfirmation(true);
  }, [selectedSessionIds, sessions]);

  const handleDuplicateSession = useCallback(
    async (session: SessionListItem) => {
      try {
        await acpForkSession(session.id);
        toast.success(intl.formatMessage(i18n.duplicateSuccess, { name: session.name }));
        window.dispatchEvent(new CustomEvent(AppEvents.SESSION_CREATED));
        await loadSessions();
      } catch (error) {
        console.error('Error duplicating session:', error);
        toast.error(
          intl.formatMessage(i18n.duplicateFailed, { error: errorMessage(error, 'Unknown error') })
        );
      }
    },
    [loadSessions, intl]
  );

  const handleConfirmDelete = useCallback(async () => {
    if (sessionsToDelete.length === 0 || isDeletingSessions) return;

    setIsDeletingSessions(true);
    const deletingSessions = sessionsToDelete;

    try {
      const results = await Promise.allSettled(
        deletingSessions.map(async (session) => {
          await acpDeleteSession(session.id);
          return session;
        })
      );
      const deletedSessions = results.flatMap((result) =>
        result.status === 'fulfilled' ? [result.value] : []
      );
      const failedResults = results.filter(
        (result): result is PromiseRejectedResult => result.status === 'rejected'
      );

      if (deletedSessions.length > 0) {
        const deletedSessionIds = new Set(deletedSessions.map((session) => session.id));
        setSessions((previous) => previous.filter((session) => !deletedSessionIds.has(session.id)));
        setSelectedSessionIds(
          (previous) =>
            new Set([...previous].filter((sessionId) => !deletedSessionIds.has(sessionId)))
        );
        for (const session of deletedSessions) {
          window.dispatchEvent(
            new CustomEvent(AppEvents.SESSION_DELETED, { detail: { sessionId: session.id } })
          );
          cancelAcpPermissionRequestsForSession(session.id);
          cancelAcpElicitationRequestsForSession(session.id);
          acpChatSessionActions.deleteSnapshot(session.id);
        }
        toast.success(
          deletedSessions.length === 1
            ? intl.formatMessage(i18n.deleteSuccess)
            : intl.formatMessage(i18n.deleteManySuccess, { count: deletedSessions.length })
        );
      }

      if (failedResults.length > 0) {
        const firstFailure = failedResults[0]?.reason;
        console.error('Error deleting sessions:', firstFailure);
        toast.error(
          deletingSessions.length === 1
            ? intl.formatMessage(i18n.deleteFailed, {
                name: deletingSessions[0]?.name ?? '',
                error: errorMessage(firstFailure, 'Unknown error'),
              })
            : intl.formatMessage(i18n.deleteManyFailed, { count: failedResults.length })
        );
      }
      await loadSessions();
    } finally {
      setIsDeletingSessions(false);
      setShowDeleteConfirmation(false);
      setSessionsToDelete([]);
    }
  }, [isDeletingSessions, sessionsToDelete, loadSessions, intl]);

  const handleCancelDelete = useCallback(() => {
    if (isDeletingSessions) return;
    setShowDeleteConfirmation(false);
    setSessionsToDelete([]);
  }, [isDeletingSessions]);

  const handleExportSession = useCallback(
    async (session: SessionListItem, e: React.MouseEvent) => {
      e.stopPropagation();

      const json = await acpExportSession(session.id);
      const blob = new Blob([json], { type: 'application/json' });
      const url = URL.createObjectURL(blob);
      const a = document.createElement('a');
      a.href = url;
      a.download = `${session.name}.json`;
      document.body.appendChild(a);
      a.click();
      document.body.removeChild(a);
      URL.revokeObjectURL(url);
      toast.success(intl.formatMessage(i18n.exportSuccess));
    },
    [intl]
  );

  const handleShareSessionNostr = useCallback(
    async (session: SessionListItem, e: React.MouseEvent) => {
      e.stopPropagation();
      setSharingSessionId(session.id);
      try {
        const response = await acpShareSessionNostr(session.id, []);
        setShareLink(response.deeplink);
        setShowShareLinkModal(true);
        toast.success(intl.formatMessage(i18n.shareNostrSuccess));
      } catch (error) {
        toast.error(
          intl.formatMessage(i18n.shareNostrFailed, { error: errorMessage(error, 'Unknown error') })
        );
      } finally {
        setSharingSessionId(null);
      }
    },
    [intl]
  );

  const handleImportClick = useCallback(async () => {
    const native = window.electron?.selectImportSessionFile;
    if (typeof native === 'function') {
      try {
        const result = await native();
        if (!result) return;
        if (result.error) {
          toast.error(intl.formatMessage(i18n.importFailed, { error: result.error }));
          return;
        }
        await acpImportSession(result.contents, 'json');
        toast.success(intl.formatMessage(i18n.importSuccess));
        window.dispatchEvent(new CustomEvent(AppEvents.SESSION_CREATED));
        await loadSessions();
      } catch (error) {
        toast.error(
          intl.formatMessage(i18n.importFailed, { error: errorMessage(error, 'Unknown error') })
        );
      }
      return;
    }
    // Fallback for non-Electron contexts (tests, web build).
    fileInputRef.current?.click();
  }, [intl, loadSessions]);

  const handleImportNostrLink = useCallback(async () => {
    const deeplink = nostrImportLink.trim();
    if (!deeplink) return;

    setIsImportingNostr(true);
    try {
      await acpImportSession(deeplink, 'nostr');
      setNostrImportLink('');
      setShowImportLinkModal(false);
      toast.success(intl.formatMessage(i18n.importSuccess));
      window.dispatchEvent(new CustomEvent(AppEvents.SESSION_CREATED));
      await loadSessions();
    } catch (error) {
      toast.error(
        intl.formatMessage(i18n.importFailed, { error: errorMessage(error, 'Unknown error') })
      );
    } finally {
      setIsImportingNostr(false);
    }
  }, [intl, loadSessions, nostrImportLink]);

  const handleCopyShareLink = useCallback(async () => {
    try {
      await navigator.clipboard.writeText(shareLink);
      toast.success(intl.formatMessage(i18n.copied));
    } catch (error) {
      toast.error(`Failed to copy: ${errorMessage(error, 'Unknown error')}`);
    }
  }, [intl, shareLink]);

  const handleImportSession = useCallback(
    async (e: React.ChangeEvent<HTMLInputElement>) => {
      const file = e.target.files?.[0];
      if (!file) return;

      try {
        const json = await file.text();
        await acpImportSession(json, 'json');

        toast.success(intl.formatMessage(i18n.importSuccess));
        window.dispatchEvent(new CustomEvent(AppEvents.SESSION_CREATED));
        await loadSessions();
      } catch (error) {
        toast.error(intl.formatMessage(i18n.importFailed, { error: String(error) }));
      } finally {
        if (fileInputRef.current) {
          fileInputRef.current.value = '';
        }
      }
    },
    [loadSessions, intl]
  );

  const handleOpenInNewWindow = useCallback((session: SessionListItem, e: React.MouseEvent) => {
    e.stopPropagation();
    window.electron.createChatWindow({
      dir: session.workingDir,
      resumeSessionId: session.id,
      viewType: 'pair',
    });
  }, []);

  const SessionItem = React.memo(function SessionItem({
    session,
    onEditClick,
    onDuplicateClick,
    onDeleteClick,
    onExportClick,
    onShareClick,
    onOpenInNewWindow,
    isSharing,
    isSelectionMode,
    isSelected,
    onToggleSelection,
  }: {
    session: SessionListItem;
    onEditClick: (session: SessionListItem) => void;
    onDuplicateClick: (session: SessionListItem) => void;
    onDeleteClick: (session: SessionListItem) => void;
    onExportClick: (session: SessionListItem, e: React.MouseEvent) => void;
    onShareClick: (session: SessionListItem, e: React.MouseEvent) => void;
    onOpenInNewWindow: (session: SessionListItem, e: React.MouseEvent) => void;
    isSharing: boolean;
    isSelectionMode: boolean;
    isSelected: boolean;
    onToggleSelection: (sessionId: string) => void;
  }) {
    const handleEditClick = useCallback(
      (e: React.MouseEvent) => {
        e.stopPropagation();
        onEditClick(session);
      },
      [onEditClick, session]
    );

    const handleDuplicateClick = useCallback(
      (e: React.MouseEvent) => {
        e.stopPropagation();
        onDuplicateClick(session);
      },
      [onDuplicateClick, session]
    );

    const handleDeleteClick = useCallback(
      (e: React.MouseEvent) => {
        e.stopPropagation();
        onDeleteClick(session);
      },
      [onDeleteClick, session]
    );

    const handleCardClick = useCallback(() => {
      if (isSelectionMode) {
        onToggleSelection(session.id);
      } else {
        onSelectSession(session.id);
      }
    }, [isSelectionMode, onToggleSelection, session.id]);

    const handleToggleSelectionClick = useCallback(
      (event: React.MouseEvent) => {
        event.stopPropagation();
        onToggleSelection(session.id);
      },
      [onToggleSelection, session.id]
    );

    const handleExportClick = useCallback(
      (e: React.MouseEvent) => {
        onExportClick(session, e);
      },
      [onExportClick, session]
    );

    const handleShareClick = useCallback(
      (e: React.MouseEvent) => {
        onShareClick(session, e);
      },
      [onShareClick, session]
    );

    const handleOpenInNewWindowClick = useCallback(
      (e: React.MouseEvent) => {
        onOpenInNewWindow(session, e);
      },
      [onOpenInNewWindow, session]
    );

    const displayName = session.name;

    return (
      <Card
        onClick={handleCardClick}
        className={`h-full py-3 px-4 hover:shadow-default cursor-pointer transition-all duration-150 flex flex-col justify-between relative group ${
          isSelectionMode && isSelected ? 'ring-2 ring-border-active' : ''
        }`}
      >
        {isSelectionMode && (
          <button
            type="button"
            onClick={handleToggleSelectionClick}
            className="absolute right-3 top-3 rounded p-1 text-text-secondary hover:bg-background-secondary hover:text-text-primary"
            title={intl.formatMessage(isSelected ? i18n.deselectSession : i18n.selectSession, {
              name: session.name,
            })}
            aria-label={intl.formatMessage(isSelected ? i18n.deselectSession : i18n.selectSession, {
              name: session.name,
            })}
            aria-pressed={isSelected}
          >
            {isSelected ? <CheckSquare className="h-5 w-5" /> : <Square className="h-5 w-5" />}
          </button>
        )}
        <div>
          <h3 className="text-base break-words line-clamp-2 w-full mb-1">{displayName}</h3>
          <div className="flex-1 mt-2">
            <div className="flex items-center text-text-secondary text-xs">
              <Calendar className="w-3 h-3 mr-1 flex-shrink-0" />
              <span>{formatMessageTimestamp(Date.parse(sessionActivityAt(session)) / 1000)}</span>
            </div>
            <div className="flex items-center text-text-secondary text-xs">
              <Folder className="w-3 h-3 mr-1 flex-shrink-0" />
              <span className="truncate">{session.workingDir}</span>
            </div>
          </div>
        </div>
        <div className="flex items-center justify-between mt-1">
          <div className="flex items-center space-x-3 text-xs text-text-secondary">
            <div className="flex items-center">
              <MessageSquareText className="w-3 h-3 mr-1" />
              <span className="font-mono">{session.messageCount}</span>
            </div>
          </div>
        </div>
        {!isSelectionMode && (
          <div className="flex justify-end gap-1 opacity-0 group-hover:opacity-100 transition-opacity">
            <button
              onClick={handleOpenInNewWindowClick}
              className="p-2 rounded hover:bg-gray-100 dark:hover:bg-gray-700 cursor-pointer"
              title={intl.formatMessage(i18n.openInNewWindow)}
            >
              <ExternalLink className="w-3 h-3 text-text-secondary hover:text-text-primary" />
            </button>
            <button
              onClick={handleEditClick}
              className="p-2 rounded hover:bg-gray-100 dark:hover:bg-gray-700 cursor-pointer"
              title={intl.formatMessage(i18n.editSessionName)}
            >
              <Edit2 className="w-3 h-3 text-text-secondary hover:text-text-primary" />
            </button>
            <button
              onClick={handleDuplicateClick}
              className="p-2 rounded hover:bg-gray-100 dark:hover:bg-gray-700 cursor-pointer"
              title={intl.formatMessage(i18n.duplicateSession)}
            >
              <Copy className="w-3 h-3 text-text-secondary hover:text-text-primary" />
            </button>
            <button
              onClick={handleDeleteClick}
              className="p-2 rounded hover:bg-red-50 dark:hover:bg-red-900/20 cursor-pointer transition-colors"
              title={intl.formatMessage(i18n.deleteSession)}
            >
              <Trash2 className="w-3 h-3 text-red-500 hover:text-red-600" />
            </button>
            <button
              onClick={handleExportClick}
              className="p-2 rounded hover:bg-gray-100 dark:hover:bg-gray-700 cursor-pointer"
              title={intl.formatMessage(i18n.exportSession)}
            >
              <Download className="w-3 h-3 text-text-secondary hover:text-text-primary" />
            </button>
            {nostrEnabled && (
              <button
                onClick={handleShareClick}
                disabled={isSharing}
                className="p-2 rounded hover:bg-gray-100 dark:hover:bg-gray-700 cursor-pointer disabled:cursor-wait disabled:opacity-60"
                title={intl.formatMessage(i18n.shareNostrSession)}
              >
                {isSharing ? (
                  <LoaderCircle className="w-3 h-3 text-text-secondary animate-spin" />
                ) : (
                  <Share2 className="w-3 h-3 text-text-secondary hover:text-text-primary" />
                )}
              </button>
            )}
          </div>
        )}
      </Card>
    );
  });

  const SessionSkeleton = React.memo(({ variant = 0 }: { variant?: number }) => {
    const titleWidths = ['w-3/4', 'w-2/3', 'w-4/5', 'w-1/2'];
    const pathWidths = ['w-32', 'w-28', 'w-36', 'w-24'];
    const tokenWidths = ['w-12', 'w-10', 'w-14', 'w-8'];

    return (
      <Card className="session-skeleton h-full py-3 px-4 flex flex-col justify-between">
        <div className="flex-1">
          <Skeleton className={`h-5 ${titleWidths[variant % titleWidths.length]} mb-2`} />
          <div className="flex items-center mb-1">
            <Skeleton className="h-3 w-3 mr-1 rounded-sm" />
            <Skeleton className="h-4 w-20" />
          </div>
          <div className="flex items-center mb-1">
            <Skeleton className="h-3 w-3 mr-1 rounded-sm" />
            <Skeleton className={`h-4 ${pathWidths[variant % pathWidths.length]}`} />
          </div>
        </div>

        <div className="flex items-center justify-between mt-1 pt-2">
          <div className="flex items-center space-x-3">
            <div className="flex items-center">
              <Skeleton className="h-3 w-3 mr-1 rounded-sm" />
              <Skeleton className="h-4 w-8" />
            </div>
            <div className="flex items-center">
              <Skeleton className="h-3 w-3 mr-1 rounded-sm" />
              <Skeleton className={`h-4 ${tokenWidths[variant % tokenWidths.length]}`} />
            </div>
          </div>
        </div>
      </Card>
    );
  });

  SessionSkeleton.displayName = 'SessionSkeleton';

  const renderActualContent = () => {
    if (error) {
      return (
        <div className="flex flex-col items-center justify-center h-full text-text-secondary">
          <AlertCircle className="h-12 w-12 text-red-500 mb-4" />
          <p className="text-lg mb-2">{intl.formatMessage(i18n.errorLoading)}</p>
          <p className="text-sm text-center mb-4">{error}</p>
          <Button onClick={() => loadSessions(debouncedSearchTerm)} variant="default">
            {intl.formatMessage(i18n.tryAgain)}
          </Button>
        </div>
      );
    }

    if (sessions.length === 0) {
      // `sessions` holds the keyword-filtered set, so an empty result while searching
      // means "no matches" rather than "no sessions at all".
      if (debouncedSearchTerm) {
        return (
          <div className="flex flex-col items-center justify-center h-full text-text-secondary mt-4">
            <MessageSquareText className="h-12 w-12 mb-4" />
            <p className="text-lg mb-2">{intl.formatMessage(i18n.noMatching)}</p>
            <p className="text-sm">{intl.formatMessage(i18n.noMatchingDesc)}</p>
          </div>
        );
      }
      return (
        <div className="flex flex-col justify-center h-full text-text-secondary">
          <MessageSquareText className="h-12 w-12 mb-4" />
          <p className="text-lg mb-2">{intl.formatMessage(i18n.noSessions)}</p>
          <p className="text-sm">{intl.formatMessage(i18n.noSessionsDesc)}</p>
        </div>
      );
    }

    return (
      <div className="space-y-8">
        {visibleDateGroups.map((group) => (
          <div key={group.label} className="space-y-4">
            <div className="sticky top-0 z-10 bg-background-primary/95 backdrop-blur-sm">
              <h2 className="text-text-secondary">{group.label}</h2>
            </div>
            <div className="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-3 xl:grid-cols-4 2xl:grid-cols-5 gap-4">
              {group.sessions.map((session) => (
                <SessionItem
                  key={session.id}
                  session={session}
                  onEditClick={handleEditSession}
                  onDuplicateClick={handleDuplicateSession}
                  onDeleteClick={handleDeleteSession}
                  onExportClick={handleExportSession}
                  onShareClick={handleShareSessionNostr}
                  onOpenInNewWindow={handleOpenInNewWindow}
                  isSharing={sharingSessionId === session.id}
                  isSelectionMode={isSelectionMode}
                  isSelected={selectedSessionIds.has(session.id)}
                  onToggleSelection={handleToggleSessionSelection}
                />
              ))}
            </div>
          </div>
        ))}

        {isPrefetchingSessions && (
          <div className="flex justify-center py-8">
            <div className="flex items-center space-x-2 text-text-secondary">
              <div className="animate-spin rounded-full h-4 w-4 border-b-2"></div>
              <span>{intl.formatMessage(i18n.loadingMore)}</span>
            </div>
          </div>
        )}
      </div>
    );
  };

  return (
    <>
      <MainPanelLayout>
        <div className="flex-1 flex flex-col min-h-0">
          <div className="bg-background-primary px-8 pb-8 pt-16">
            <div className="flex flex-col page-transition">
              <div className="flex justify-between items-center mb-1">
                <h1 className="text-4xl font-light">{intl.formatMessage(i18n.chatHistory)}</h1>
                <div className="flex items-center gap-2">
                  {nostrEnabled && (
                    <Button
                      onClick={() => setShowImportLinkModal(true)}
                      variant="outline"
                      size="sm"
                      className="flex items-center gap-2"
                    >
                      <Share2 className="w-4 h-4" />
                      {intl.formatMessage(i18n.importNostrSession)}
                    </Button>
                  )}
                  <Button
                    onClick={handleImportClick}
                    variant="outline"
                    size="sm"
                    className="flex items-center gap-2"
                  >
                    <Upload className="w-4 h-4" />
                    {intl.formatMessage(i18n.importSession)}
                  </Button>
                  <Button
                    onClick={() => handleSelectionModeChange(!isSelectionMode)}
                    variant="outline"
                    size="sm"
                    className="flex items-center gap-2"
                  >
                    {isSelectionMode ? (
                      <Square className="w-4 h-4" />
                    ) : (
                      <CheckSquare className="w-4 h-4" />
                    )}
                    {intl.formatMessage(
                      isSelectionMode ? i18n.cancelSelection : i18n.selectSessions
                    )}
                  </Button>
                </div>
              </div>
              <p className="text-sm text-text-secondary mb-4">
                {intl.formatMessage(i18n.chatHistoryDesc, { shortcut: getSearchShortcutText() })}
              </p>
              {isSelectionMode && (
                <div className="flex flex-wrap items-center gap-2 rounded-lg border border-border-primary bg-background-secondary px-3 py-2 text-sm">
                  <span className="mr-auto text-text-secondary" aria-live="polite">
                    {intl.formatMessage(i18n.selectedCount, { count: selectedSessionIds.size })}
                  </span>
                  <Button
                    onClick={handleSelectAllVisibleSessions}
                    variant="outline"
                    size="sm"
                    disabled={sessions.length === 0 || isDeletingSessions}
                  >
                    {intl.formatMessage(
                      allVisibleSessionsSelected ? i18n.clearSelection : i18n.selectAll
                    )}
                  </Button>
                  <Button
                    onClick={handleDeleteSelectedSessions}
                    variant="destructive"
                    size="sm"
                    disabled={selectedSessionIds.size === 0 || isDeletingSessions}
                  >
                    <Trash2 className="w-4 h-4" />
                    {intl.formatMessage(i18n.deleteSelected)}
                  </Button>
                </div>
              )}
            </div>
          </div>

          <div className="flex-1 min-h-0 relative">
            <ScrollArea handleScroll={handleScroll} className="h-full" data-search-scroll-area>
              <div ref={containerRef} className="h-full relative px-8">
                <SearchView
                  onSearch={handleSearch}
                  className="relative"
                  placeholder={intl.formatMessage(i18n.searchPlaceholder)}
                  showCaseSensitive={false}
                  showNavigation={false}
                  highlightMatches={false}
                >
                  {/* Skeleton layer - always rendered but conditionally visible */}
                  <div
                    className={`absolute inset-0 transition-opacity duration-300 ${
                      isLoading || showSkeleton
                        ? 'opacity-100 z-10'
                        : 'opacity-0 z-0 pointer-events-none'
                    }`}
                  >
                    <div className="space-y-8">
                      {/* Today section */}
                      <div className="space-y-4">
                        <Skeleton className="h-6 w-16" />
                        <div className="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-3 xl:grid-cols-4 2xl:grid-cols-5 gap-4">
                          <SessionSkeleton variant={0} />
                          <SessionSkeleton variant={1} />
                          <SessionSkeleton variant={2} />
                          <SessionSkeleton variant={3} />
                          <SessionSkeleton variant={0} />
                        </div>
                      </div>

                      {/* Yesterday section */}
                      <div className="space-y-4">
                        <Skeleton className="h-6 w-20" />
                        <div className="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-3 xl:grid-cols-4 2xl:grid-cols-5 gap-4">
                          <SessionSkeleton variant={1} />
                          <SessionSkeleton variant={2} />
                          <SessionSkeleton variant={3} />
                          <SessionSkeleton variant={0} />
                          <SessionSkeleton variant={1} />
                          <SessionSkeleton variant={2} />
                        </div>
                      </div>

                      {/* Additional section */}
                      <div className="space-y-4">
                        <Skeleton className="h-6 w-24" />
                        <div className="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-3 xl:grid-cols-4 2xl:grid-cols-5 gap-4">
                          <SessionSkeleton variant={3} />
                          <SessionSkeleton variant={0} />
                          <SessionSkeleton variant={1} />
                        </div>
                      </div>
                    </div>
                  </div>

                  {/* Content layer - always rendered but conditionally visible */}
                  <div
                    className={`relative transition-opacity duration-300 ${
                      showContent ? 'opacity-100 z-10' : 'opacity-0 z-0'
                    }`}
                  >
                    {renderActualContent()}
                  </div>
                </SearchView>
              </div>
            </ScrollArea>
          </div>
        </div>
      </MainPanelLayout>

      <input
        ref={fileInputRef}
        type="file"
        accept=".json,.jsonl,application/json,application/x-ndjson"
        onChange={handleImportSession}
        className="hidden"
      />

      <EditSessionModal
        session={editingSession}
        isOpen={showEditModal}
        onClose={handleModalClose}
        onSave={handleModalSave}
      />

      <Dialog open={showImportLinkModal} onOpenChange={setShowImportLinkModal}>
        <DialogContent className="sm:max-w-lg">
          <DialogHeader>
            <DialogTitle className="flex items-center gap-2">
              <Share2 className="w-5 h-5" />
              {intl.formatMessage(i18n.importNostrTitle)}
            </DialogTitle>
            <DialogDescription>{intl.formatMessage(i18n.importNostrDesc)}</DialogDescription>
          </DialogHeader>

          <textarea
            value={nostrImportLink}
            onChange={(event) => setNostrImportLink(event.target.value)}
            placeholder={intl.formatMessage(i18n.importNostrPlaceholder)}
            className="min-h-28 w-full resize-none rounded-lg border border-border-primary bg-background-primary p-3 text-sm text-text-primary outline-none focus:ring-2 focus:ring-border-active"
            disabled={isImportingNostr}
          />

          <DialogFooter>
            <Button
              variant="outline"
              onClick={() => setShowImportLinkModal(false)}
              disabled={isImportingNostr}
            >
              {intl.formatMessage(i18n.cancel)}
            </Button>
            <Button
              onClick={handleImportNostrLink}
              disabled={isImportingNostr || !nostrImportLink.trim()}
            >
              {isImportingNostr ? (
                <>
                  <LoaderCircle className="w-4 h-4 animate-spin" />
                  {intl.formatMessage(i18n.importing)}
                </>
              ) : (
                intl.formatMessage(i18n.importSession)
              )}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      <Dialog open={showShareLinkModal} onOpenChange={setShowShareLinkModal}>
        <DialogContent className="sm:max-w-lg">
          <DialogHeader>
            <DialogTitle className="flex items-center gap-2">
              <Share2 className="w-5 h-5" />
              {intl.formatMessage(i18n.shareNostrTitle)}
            </DialogTitle>
            <DialogDescription>{intl.formatMessage(i18n.shareNostrDesc)}</DialogDescription>
          </DialogHeader>

          <div className="relative rounded-lg border border-border-primary bg-background-secondary p-3 pr-12">
            <code className="block max-h-36 overflow-y-auto break-all text-sm text-text-primary">
              {shareLink}
            </code>
            <Button
              variant="ghost"
              size="sm"
              className="absolute right-2 top-2"
              onClick={handleCopyShareLink}
              disabled={!shareLink}
            >
              <Copy className="h-4 w-4" />
              <span className="sr-only">{intl.formatMessage(i18n.copied)}</span>
            </Button>
          </div>

          <DialogFooter>
            <Button variant="outline" onClick={() => setShowShareLinkModal(false)}>
              {intl.formatMessage(i18n.close)}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      <ConfirmationModal
        isOpen={showDeleteConfirmation}
        title={intl.formatMessage(
          sessionsToDelete.length > 1 ? i18n.deleteManyTitle : i18n.deleteTitle,
          { count: sessionsToDelete.length }
        )}
        message={intl.formatMessage(
          sessionsToDelete.length > 1 ? i18n.deleteManyMessage : i18n.deleteMessage,
          sessionsToDelete.length > 1
            ? { count: sessionsToDelete.length }
            : { name: sessionsToDelete[0]?.name ?? '' }
        )}
        confirmLabel={intl.formatMessage(
          sessionsToDelete.length > 1 ? i18n.deleteManyTitle : i18n.deleteTitle,
          { count: sessionsToDelete.length }
        )}
        cancelLabel={intl.formatMessage(i18n.cancel)}
        confirmVariant="destructive"
        isSubmitting={isDeletingSessions}
        onConfirm={handleConfirmDelete}
        onCancel={handleCancelDelete}
      />
    </>
  );
});

SessionListView.displayName = 'SessionListView';

export default SessionListView;
