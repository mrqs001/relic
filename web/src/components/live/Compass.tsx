/** Course over ground as an arrow and the bow's heading as the hull, on a
 *  compass rose. A stopped vessel shows its hull without a course arrow. */
export default function Compass({ course, heading, moving, color }: { course: number | null; heading: number | null; moving: boolean; color: string }) {
  const hull = heading ?? course;
  const ticks = Array.from({ length: 36 }, (_, i) => i * 10);
  return <svg className="live-compass" viewBox="-50 -50 100 100" role="img"
    aria-label={[course != null && moving ? `Course ${Math.round(course)}°` : null, heading != null ? `heading ${Math.round(heading)}°` : null].filter(Boolean).join(", ") || "Direction not reported"}>
    <circle r="46" className="live-compass-face"/>
    {ticks.map(d => <line key={d} className={d % 90 === 0 ? "major" : d % 30 === 0 ? "mid" : ""} x1="0" x2="0" y1={-46} y2={d % 90 === 0 ? -38 : d % 30 === 0 ? -41 : -43.5} transform={`rotate(${d})`}/>)}
    <text x="0" y="-27" className="live-compass-n">N</text>
    {course != null && moving && <g transform={`rotate(${course})`}>
      <line x1="0" y1="0" x2="0" y2="-36" className="live-compass-course"/>
      <path d="M0,-44 L-4.5,-35 L4.5,-35 Z" className="live-compass-tip"/>
    </g>}
    {hull != null
      ? <path transform={`rotate(${hull})`} d="M0,-17 C5,-11 6,-4 6,3 L5.5,15 L-5.5,15 L-6,3 C-6,-4 -5,-11 0,-17 Z" fill={color} className="live-compass-hull"/>
      : <circle r="6" fill={color} className="live-compass-hull"/>}
  </svg>;
}
