import ponduinIcon from '../images/ponduin-icon.png';
import { cn } from '../utils';

interface PonduinLogoProps {
  className?: string;
  size?: 'default' | 'small';
  hover?: boolean;
}

export default function PonduinLogo({ className = '', size = 'default' }: PonduinLogoProps) {
  return (
    <img
      src={ponduinIcon}
      alt="Ponduin"
      className={cn(
        size === 'default' ? 'size-16' : 'size-8',
        'object-cover rounded-[22%]',
        className
      )}
    />
  );
}
