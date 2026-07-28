import { ModeSection } from '../mode/ModeSection';
import { DictationSettings } from '../dictation/DictationSettings';
import { SecurityToggle } from '../security/SecurityToggle';
import { ResponseStylesSection } from '../response_styles/ResponseStylesSection';
import { PonduinhintsSection } from './PonduinhintsSection';
import { SpellcheckToggle } from './SpellcheckToggle';
import { CodingAgentSettings } from './CodingAgentSettings';
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '../../ui/card';
import { defineMessages, useIntl } from '../../../i18n';

const i18n = defineMessages({
  modeTitle: {
    id: 'chatSettings.modeTitle',
    defaultMessage: 'Default Mode',
  },
  modeDescription: {
    id: 'chatSettings.modeDescription',
    defaultMessage:
      'Choose the default mode Ponduin uses for new sessions. Existing sessions keep their current mode.',
  },
  codingAgentTitle: {
    id: 'chatSettings.codingAgentTitle',
    defaultMessage: 'Internal Coding Agent',
  },
  codingAgentDescription: {
    id: 'chatSettings.codingAgentDescription',
    defaultMessage:
      'Enable provider-independent coding capabilities and choose the workflow for new tasks.',
  },
  responseStylesTitle: {
    id: 'chatSettings.responseStylesTitle',
    defaultMessage: 'Response Styles',
  },
  responseStylesDescription: {
    id: 'chatSettings.responseStylesDescription',
    defaultMessage: 'Choose how Ponduin should format and style its responses',
  },
});

export default function ChatSettingsSection() {
  const intl = useIntl();

  return (
    <div className="space-y-4 pr-4 pb-8 mt-1">
      <Card className="pb-2 rounded-lg">
        <CardHeader className="pb-0">
          <CardTitle className="">{intl.formatMessage(i18n.modeTitle)}</CardTitle>
          <CardDescription>{intl.formatMessage(i18n.modeDescription)}</CardDescription>
        </CardHeader>
        <CardContent className="px-2">
          <ModeSection />
        </CardContent>
      </Card>

      <Card className="pb-2 rounded-lg">
        <CardHeader className="pb-0">
          <CardTitle>{intl.formatMessage(i18n.codingAgentTitle)}</CardTitle>
          <CardDescription>{intl.formatMessage(i18n.codingAgentDescription)}</CardDescription>
        </CardHeader>
        <CardContent className="px-2">
          <CodingAgentSettings />
        </CardContent>
      </Card>

      <Card className="pb-2 rounded-lg">
        <CardContent className="px-2">
          <PonduinhintsSection />
        </CardContent>
      </Card>

      <Card className="pb-2 rounded-lg">
        <CardContent className="px-2">
          <DictationSettings />
          <SpellcheckToggle />
        </CardContent>
      </Card>

      <Card className="pb-2 rounded-lg">
        <CardHeader className="pb-0">
          <CardTitle className="">{intl.formatMessage(i18n.responseStylesTitle)}</CardTitle>
          <CardDescription>{intl.formatMessage(i18n.responseStylesDescription)}</CardDescription>
        </CardHeader>
        <CardContent className="px-2">
          <ResponseStylesSection />
        </CardContent>
      </Card>

      <Card className="pb-2 rounded-lg">
        <CardContent className="px-2">
          <SecurityToggle />
        </CardContent>
      </Card>
    </div>
  );
}
