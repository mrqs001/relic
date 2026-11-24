import { useInfiniteQuery } from "@tanstack/react-query";
import { z } from "zod";
import { api } from "../api";
import { ErrorMessage } from "./Common";
const schema = z.object({publication_id:z.string(),next_cursor:z.string().nullable(),items:z.array(z.object({message_key:z.string(),source_label:z.string(),preview:z.string(),reason:z.string(),retained_reading:z.boolean()}))});
export default function ProcessingFailures() {
  const query=useInfiniteQuery({queryKey:["processing-failures"],initialPageParam:null as string|null,
    queryFn:({pageParam,signal})=>api(`/processing/failures${pageParam?`?before=${pageParam}`:""}`,schema,{signal}),getNextPageParam:page=>page.next_cursor});
  return <div className="processing-failures"><ErrorMessage error={query.error}/>{query.isLoading && <p>Loading messages…</p>}
    {query.data?.pages.flatMap(page=>page.items).map(item=><article key={item.message_key}>
      <a href={`/?m=${item.message_key}`}>{item.source_label} ↗</a><p>{item.preview}</p><small>{item.reason}{item.retained_reading?" · Previous reading retained":""}</small>
    </article>)}
    {query.hasNextPage && <button className="text-button" disabled={query.isFetchingNextPage} onClick={()=>void query.fetchNextPage()}>Show more</button>}
  </div>;
}
