import PonduinLogo from './components/PonduinLogo';

export default function SuspenseLoader() {
  return (
    <div className="flex flex-col items-start justify-end w-screen h-screen overflow-hidden p-6 page-transition">
      <div className="flex gap-2 items-center justify-end">
        <PonduinLogo size="small" />
        <span className="text-text-secondary">Loading...</span>
      </div>
    </div>
  );
}
