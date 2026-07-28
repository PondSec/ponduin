import React from 'react';
import ponduinIcon from '../../images/ponduin-icon.png';

type Props = React.ComponentPropsWithoutRef<'img'>;

export function PonduinBadge({ className = '', ...props }: Props) {
  return (
    <img
      src={ponduinIcon}
      alt=""
      aria-hidden="true"
      className={`rounded-[28%] object-cover ${className}`}
      {...props}
    />
  );
}
