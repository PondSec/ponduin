import ponduinIcon from '../../images/ponduin-icon.png';

export function Ponduin({ className = '' }) {
  return (
    <img
      src={ponduinIcon}
      alt=""
      aria-hidden="true"
      className={`object-cover rounded-[22%] ${className}`}
    />
  );
}
