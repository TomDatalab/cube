// An isometric cube: the top face filled faintly so it reads as a solid at
// 14px, edges in the text color so it follows the light and dark themes.
export const CubeIcon = () => (
  <svg width="1em" height="1em" viewBox="0 0 16 16" fill="none" xmlns="http://www.w3.org/2000/svg" aria-hidden="true">
    <path d="M8 1.75 14 5v6L8 14.25 2 11V5l6-3.25Z" stroke="currentColor" strokeWidth="1.3" strokeLinejoin="round" />
    <path d="M2 5l6 3.25L14 5L8 1.75 2 5Z" fill="currentColor" fillOpacity="0.18" />
    <path d="M2 5l6 3.25L14 5M8 8.25v6" stroke="currentColor" strokeWidth="1.3" strokeLinejoin="round" />
  </svg>
);
