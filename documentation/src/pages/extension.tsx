import React, { useEffect } from 'react';
import { useLocation } from '@docusaurus/router';

export default function ExtensionRedirect(): React.JSX.Element {
  const location = useLocation();

  useEffect(() => {
    const params = new URLSearchParams(location.search);
    window.location.href = `ponduin://extension${params.toString() ? '?' + params.toString() : ''}`;
  }, [location]);

  return (
    <div style={{ padding: '2rem', textAlign: 'center' }}>
      Redirecting to Ponduin...
    </div>
  );
}
