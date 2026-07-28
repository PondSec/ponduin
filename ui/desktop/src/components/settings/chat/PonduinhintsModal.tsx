import { useState, useEffect } from 'react';
import { Button } from '../../ui/button';
import { Check } from '../../icons';
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '../../ui/dialog';
import { errorMessage } from '../../../utils/conversionUtils';
import { defineMessages, useIntl } from '../../../i18n';

const i18n = defineMessages({
  dialogTitle: {
    id: 'ponduinhintsModal.dialogTitle',
    defaultMessage: 'Configure Project Hints (.ponduinhints)',
  },
  dialogDescription: {
    id: 'ponduinhintsModal.dialogDescription',
    defaultMessage:
      'Provide additional context about your project to improve communication with Ponduin',
  },
  helpText1: {
    id: 'ponduinhintsModal.helpText1',
    defaultMessage:
      '.ponduinhints is a text file used to provide additional context about your project and improve the communication with Ponduin.',
  },
  helpText2: {
    id: 'ponduinhintsModal.helpText2',
    defaultMessage:
      "Please make sure {bold} extension is enabled in the extensions page. This extension is required to use .ponduinhints. You'll need to restart your session for .ponduinhints updates to take effect.",
  },
  helpText3: {
    id: 'ponduinhintsModal.helpText3',
    defaultMessage: 'See {link} for more information.',
  },
  helpTextLink: {
    id: 'ponduinhintsModal.helpTextLink',
    defaultMessage: 'using .ponduinhints',
  },
  errorReading: {
    id: 'ponduinhintsModal.errorReading',
    defaultMessage: 'Error reading .ponduinhints file: {error}',
  },
  fileFound: {
    id: 'ponduinhintsModal.fileFound',
    defaultMessage: '.ponduinhints file found at: {filePath}',
  },
  fileCreating: {
    id: 'ponduinhintsModal.fileCreating',
    defaultMessage: 'Creating new .ponduinhints file at: {filePath}',
  },
  placeholder: {
    id: 'ponduinhintsModal.placeholder',
    defaultMessage: 'Enter project hints here...',
  },
  savedSuccessfully: {
    id: 'ponduinhintsModal.savedSuccessfully',
    defaultMessage: 'Saved successfully',
  },
  close: {
    id: 'ponduinhintsModal.close',
    defaultMessage: 'Close',
  },
  saving: {
    id: 'ponduinhintsModal.saving',
    defaultMessage: 'Saving...',
  },
  save: {
    id: 'ponduinhintsModal.save',
    defaultMessage: 'Save',
  },
  failedToAccess: {
    id: 'ponduinhintsModal.failedToAccess',
    defaultMessage: 'Failed to access .ponduinhints file',
  },
  failedToSave: {
    id: 'ponduinhintsModal.failedToSave',
    defaultMessage: 'Failed to save .ponduinhints file',
  },
  developer: {
    id: 'ponduinhintsModal.developer',
    defaultMessage: 'Developer',
  },
});

const HelpText = () => {
  const intl = useIntl();

  return (
    <div className="text-sm flex-col space-y-4 text-text-secondary">
      <p>{intl.formatMessage(i18n.helpText1)}</p>
      <p>
        {intl.formatMessage(i18n.helpText2, {
          bold: <span className="font-bold">{intl.formatMessage(i18n.developer)}</span>,
        })}
      </p>
      <p>
        {intl.formatMessage(i18n.helpText3, {
          link: (
            <Button
              variant="link"
              className="text-blue-500 hover:text-blue-600 p-0 h-auto"
              onClick={() =>
                window.open('https://ponduin.de/docs/guides/using-ponduinhints/', '_blank')
              }
            >
              {intl.formatMessage(i18n.helpTextLink)}
            </Button>
          ),
        })}
      </p>
    </div>
  );
};

const ErrorDisplay = ({ error }: { error: Error }) => {
  const intl = useIntl();

  return (
    <div className="text-sm text-text-secondary">
      <div className="text-red-600">
        {intl.formatMessage(i18n.errorReading, { error: errorMessage(error) })}
      </div>
    </div>
  );
};

const FileInfo = ({ filePath, found }: { filePath: string; found: boolean }) => {
  const intl = useIntl();

  return (
    <div className="text-sm font-medium mb-2">
      {found ? (
        <div className="text-green-600">
          <Check className="w-4 h-4 inline-block" />{' '}
          {intl.formatMessage(i18n.fileFound, { filePath })}
        </div>
      ) : (
        <div>{intl.formatMessage(i18n.fileCreating, { filePath })}</div>
      )}
    </div>
  );
};

const getPonduinhintsFile = async (filePath: string) => await window.electron.readFile(filePath);

interface PonduinhintsModalProps {
  directory: string;
  setIsPonduinhintsModalOpen: (isOpen: boolean) => void;
}

export const PonduinhintsModal = ({
  directory,
  setIsPonduinhintsModalOpen,
}: PonduinhintsModalProps) => {
  const intl = useIntl();
  const ponduinhintsFilePath = `${directory}/.ponduinhints`;
  const [ponduinhintsFile, setPonduinhintsFile] = useState<string>('');
  const [ponduinhintsFileFound, setPonduinhintsFileFound] = useState<boolean>(false);
  const [ponduinhintsFileReadError, setPonduinhintsFileReadError] = useState<string>('');
  const [isSaving, setIsSaving] = useState(false);
  const [saveSuccess, setSaveSuccess] = useState(false);

  useEffect(() => {
    const fetchPonduinhintsFile = async () => {
      try {
        const { file, error, found } = await getPonduinhintsFile(ponduinhintsFilePath);
        setPonduinhintsFile(file);
        setPonduinhintsFileFound(found);
        setPonduinhintsFileReadError(found && error ? error : '');
      } catch (error) {
        console.error('Error fetching .ponduinhints file:', error);
        setPonduinhintsFileReadError(intl.formatMessage(i18n.failedToAccess));
      }
    };
    if (directory) fetchPonduinhintsFile();
  }, [directory, ponduinhintsFilePath, intl]);

  const writeFile = async () => {
    setIsSaving(true);
    setSaveSuccess(false);
    try {
      await window.electron.writeFile(ponduinhintsFilePath, ponduinhintsFile);
      setSaveSuccess(true);
      setPonduinhintsFileFound(true);
      setTimeout(() => setSaveSuccess(false), 3000);
    } catch (error) {
      console.error('Error writing .ponduinhints file:', error);
      setPonduinhintsFileReadError(intl.formatMessage(i18n.failedToSave));
    } finally {
      setIsSaving(false);
    }
  };

  return (
    <Dialog open={true} onOpenChange={(open) => setIsPonduinhintsModalOpen(open)}>
      <DialogContent className="w-[80vw] max-w-[80vw] sm:max-w-[80vw] max-h-[90vh] flex flex-col">
        <DialogHeader>
          <DialogTitle>{intl.formatMessage(i18n.dialogTitle)}</DialogTitle>
          <DialogDescription>{intl.formatMessage(i18n.dialogDescription)}</DialogDescription>
        </DialogHeader>

        <div className="flex-1 overflow-y-auto space-y-4 pt-2 pb-4">
          <HelpText />

          <div>
            {ponduinhintsFileReadError ? (
              <ErrorDisplay error={new Error(ponduinhintsFileReadError)} />
            ) : (
              <div className="space-y-2">
                <FileInfo filePath={ponduinhintsFilePath} found={ponduinhintsFileFound} />
                <textarea
                  value={ponduinhintsFile}
                  className="w-full h-80 border rounded-md p-2 text-sm resize-none bg-background-primary text-text-primary border-border-primary focus:outline-none focus:ring-2 focus:ring-blue-500"
                  onChange={(event) => setPonduinhintsFile(event.target.value)}
                  placeholder={intl.formatMessage(i18n.placeholder)}
                />
              </div>
            )}
          </div>
        </div>

        <DialogFooter>
          {saveSuccess && (
            <span className="text-green-600 text-sm flex items-center gap-1 mr-auto">
              <Check className="w-4 h-4" />
              {intl.formatMessage(i18n.savedSuccessfully)}
            </span>
          )}
          <Button variant="outline" onClick={() => setIsPonduinhintsModalOpen(false)}>
            {intl.formatMessage(i18n.close)}
          </Button>
          <Button onClick={writeFile} disabled={isSaving}>
            {isSaving ? intl.formatMessage(i18n.saving) : intl.formatMessage(i18n.save)}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
};
