export const PonduinLogo = (props: { className?: string }) => {
  return (
    <img
      src="/img/ponduin-logo.png"
      alt="Ponduin"
      className={props.className}
      style={{ height: 'auto', maxWidth: '100%' }}
    />
  );
};
