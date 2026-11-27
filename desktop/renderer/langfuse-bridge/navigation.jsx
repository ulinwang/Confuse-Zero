import { createContext, useContext } from "react";

const Navigation = createContext(null);
const events = new EventTarget();
const router = {
  events: {
    on: (name, listener) => events.addEventListener(name, listener),
    off: (name, listener) => events.removeEventListener(name, listener),
  },
};

export function DesktopNavigation({ routes, page, navigate, children }) {
  return <Navigation.Provider value={{ routes, page, navigate }}>{children}</Navigation.Provider>;
}

export function useRouter() {
  return router;
}

export default function Link({ href, onClick, prefetch, replace, scroll, shallow, ...props }) {
  const navigation = useContext(Navigation);
  const target = typeof href === "string" ? href : href?.pathname;
  const page = navigation?.routes.get(target);
  return <a {...props} href={target} aria-current={navigation?.page === page && page ? "page" : props["aria-current"]}
    onClick={event => {
      onClick?.(event);
      if (!page || event.defaultPrevented) return;
      event.preventDefault();
      events.dispatchEvent(new Event("routeChangeStart"));
      navigation.navigate(page);
    }}/>;
}
