import { useEffect, useRef } from "react";

/** Runs `reload` each time a view that stayed mounted becomes visible again. */
export function useOnReturn(visible: boolean, reload: () => void) {
  const was = useRef(visible);
  useEffect(() => {
    if (visible && !was.current) reload();
    was.current = visible;
  }, [visible]); // eslint-disable-line react-hooks/exhaustive-deps
}
