function parse(source) {
  const doc = new DOMParser().parseFromString(source, 'image/svg+xml');
  const root = doc.documentElement;
  if (root.tagName.toLowerCase() === 'parsererror') {
    throw new Error('SVG parse failed');
  }
  return root;
}

const OPEN = '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" '
  + 'fill="none" stroke="currentColor" stroke-width="2" '
  + 'stroke-linecap="round" stroke-linejoin="round">';

const make = (body) => parse(OPEN + body + '</svg>');

const ICON_HOME = make('<path d="m3 9 9-7 9 7v11a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z"/><polyline points="9 22 9 12 15 12 15 22"/>');
const ICON_LOADS = make('<path d="M21 8a2 2 0 0 0-1-1.73l-7-4a2 2 0 0 0-2 0l-7 4A2 2 0 0 0 3 8v8a2 2 0 0 0 1 1.73l7 4a2 2 0 0 0 2 0l7-4A2 2 0 0 0 21 16Z"/><path d="m3.3 7 8.7 5 8.7-5"/><path d="M12 22V12"/>');
const ICON_TRIPS = make('<circle cx="6" cy="19" r="3"/><path d="M9 19h8.5a3.5 3.5 0 0 0 0-7h-11a3.5 3.5 0 0 1 0-7H15"/><circle cx="18" cy="5" r="3"/>');
const ICON_EVENTS = make('<path d="M22 12h-4l-3 9L9 3l-3 9H2"/>');
const ICON_DRIVERS = make('<path d="M16 21v-2a4 4 0 0 0-4-4H6a4 4 0 0 0-4 4v2"/><circle cx="9" cy="7" r="4"/><path d="M22 21v-2a4 4 0 0 0-3-3.87"/><path d="M16 3.13a4 4 0 0 1 0 7.75"/>');
const ICON_TRUCKS = make('<path d="M14 18V6a2 2 0 0 0-2-2H4a2 2 0 0 0-2 2v11a1 1 0 0 0 1 1h2"/><path d="M15 18H9"/><path d="M19 18h2a1 1 0 0 0 1-1v-3.65a1 1 0 0 0-.22-.624l-3.48-4.35A1 1 0 0 0 17.52 8H14"/><circle cx="17" cy="18" r="2"/><circle cx="7" cy="18" r="2"/>');
const ICON_TRAILERS = make('<rect x="2" y="6" width="18" height="12" rx="1"/><circle cx="8" cy="20" r="2"/><circle cx="16" cy="20" r="2"/><path d="M20 12h2"/>');
const ICON_FACILITIES = make('<rect x="4" y="2" width="16" height="20" rx="2"/><path d="M9 22v-4h6v4"/><path d="M8 6h.01"/><path d="M16 6h.01"/><path d="M12 6h.01"/><path d="M12 10h.01"/><path d="M12 14h.01"/><path d="M16 10h.01"/><path d="M8 10h.01"/>');
const ICON_TERMINALS = make('<path d="M22 8.35V20a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V8.35A2 2 0 0 1 3.26 6.5l8-3.2a2 2 0 0 1 1.48 0l8 3.2A2 2 0 0 1 22 8.35Z"/><path d="M6 18h12"/><path d="M6 14h12"/><path d="M6 10h12"/>');
const ICON_DOCUMENTS = make('<path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z"/><polyline points="14 2 14 8 20 8"/><line x1="16" y1="13" x2="8" y2="13"/><line x1="16" y1="17" x2="8" y2="17"/>');
const ICON_KEY = make('<path d="m15.5 7.5 2.3 2.3a1 1 0 0 0 1.4 0l2.1-2.1a1 1 0 0 0 0-1.4L19 4"/><path d="m21 2-9.6 9.6"/><circle cx="7.5" cy="15.5" r="5.5"/>');
const ICON_CHEV_UP = make('<polyline points="18 15 12 9 6 15"/>');
const ICON_LOGOUT = make('<path d="M9 21H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h4"/><polyline points="16 17 21 12 16 7"/><line x1="21" y1="12" x2="9" y2="12"/>');

export const homeIcon = () => ICON_HOME.cloneNode(true);
export const loadsIcon = () => ICON_LOADS.cloneNode(true);
export const tripsIcon = () => ICON_TRIPS.cloneNode(true);
export const eventsIcon = () => ICON_EVENTS.cloneNode(true);
export const driversIcon = () => ICON_DRIVERS.cloneNode(true);
export const trucksIcon = () => ICON_TRUCKS.cloneNode(true);
export const trailersIcon = () => ICON_TRAILERS.cloneNode(true);
export const facilitiesIcon = () => ICON_FACILITIES.cloneNode(true);
export const terminalsIcon = () => ICON_TERMINALS.cloneNode(true);
export const documentsIcon = () => ICON_DOCUMENTS.cloneNode(true);
export const keyIcon = () => ICON_KEY.cloneNode(true);
export const chevronUpIcon = () => ICON_CHEV_UP.cloneNode(true);
export const logoutIcon = () => ICON_LOGOUT.cloneNode(true);
