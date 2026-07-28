export const getInitialWorkingDir = (): string => {
  // Fall back to initial config from app startup
  return (window.appConfig?.get('PONDUIN_WORKING_DIR') as string) ?? '';
};
