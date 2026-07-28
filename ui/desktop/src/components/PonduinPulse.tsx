import { Ponduin } from './icons';

interface PonduinPulseProps {
  className?: string;
  cycleInterval?: number;
}

export default function PonduinPulse({ className = '', cycleInterval = 600 }: PonduinPulseProps) {
  return (
    <div
      className={`animate-pulse ${className}`}
      style={{ animationDuration: `${Math.max(cycleInterval, 300)}ms` }}
    >
      <Ponduin className="size-4" />
    </div>
  );
}
