import { getPageImage, source } from "@/lib/source";
import { notFound } from "next/navigation";
import { ImageResponse } from "next/og";

export const revalidate = false;

async function loadGoogleFont(font: string, text: string) {
  const url = `https://fonts.googleapis.com/css2?family=${font}&text=${encodeURIComponent(text)}`;
  const css = await (await fetch(url)).text();
  const resource = css.match(/src: url\((.+)\) format\('(opentype|truetype)'\)/);

  if (resource) {
    const response = await fetch(resource[1]);
    if (response.status == 200) {
      return await response.arrayBuffer();
    }
  }

  throw new Error("failed to load font data");
}

export async function GET(_req: Request, { params }: RouteContext<"/og/docs/[...slug]">) {
  const { slug } = await params;
  const page = source.getPage(slug.slice(0, -1));
  if (!page) notFound();

  return new ImageResponse(
    <div
      style={{
        height: "100%",
        width: "100%",
        display: "flex",
        padding: "6rem",
        flexDirection: "column",
        alignItems: "flex-start",
        justifyContent: "center",
        backgroundColor: "#1A1A1A",
        color: "#fff",
      }}
    >
      <svg
        width="116"
        height="31"
        viewBox="0 0 116 31"
        fill="none"
        xmlns="http://www.w3.org/2000/svg"
      >
        <path
          d="M53.1411 26.4276C50.4938 26.4276 48.3964 25.6945 46.8487 24.2284C45.3011 22.7418 44.5273 20.624 44.5273 17.8749V12.7433C44.5273 9.99418 45.3011 7.88655 46.8487 6.42036C48.3964 4.93382 50.4938 4.19054 53.1411 4.19054C55.768 4.19054 57.7942 4.91345 59.2196 6.35927C60.6655 7.78473 61.3884 9.74982 61.3884 12.2545V12.4378H57.4175V12.1324C57.4175 10.8698 57.0611 9.83127 56.3484 9.01673C55.656 8.20218 54.5869 7.79491 53.1411 7.79491C51.7156 7.79491 50.5956 8.23273 49.7811 9.10836C48.9665 9.984 48.5593 11.1753 48.5593 12.6822V17.936C48.5593 19.4225 48.9665 20.6138 49.7811 21.5098C50.5956 22.3855 51.7156 22.8233 53.1411 22.8233C54.5869 22.8233 55.656 22.416 56.3484 21.6015C57.0611 20.7665 57.4175 19.728 57.4175 18.4858V17.936H61.3884V18.3636C61.3884 20.8684 60.6655 22.8436 59.2196 24.2895C57.7942 25.7149 55.768 26.4276 53.1411 26.4276ZM64.7959 26V10.8495H68.5835V12.8349H69.1333C69.3777 12.3055 69.8359 11.8065 70.5079 11.3382C71.1799 10.8495 72.198 10.6051 73.5624 10.6051C74.7435 10.6051 75.7719 10.88 76.6475 11.4298C77.5435 11.9593 78.2359 12.7025 78.7246 13.6596C79.2133 14.5964 79.4577 15.696 79.4577 16.9585V26H75.6089V17.264C75.6089 16.1236 75.3239 15.2684 74.7537 14.6982C74.2039 14.128 73.4097 13.8429 72.3711 13.8429C71.19 13.8429 70.2737 14.24 69.622 15.0342C68.9704 15.808 68.6446 16.8975 68.6446 18.3025V26H64.7959ZM89.0043 26C88.0065 26 87.192 25.6945 86.5607 25.0836C85.9498 24.4524 85.6443 23.6175 85.6443 22.5789V14.0262H81.8567V10.8495H85.6443V6.14545H89.493V10.8495H93.6472V14.0262H89.493V21.9069C89.493 22.5178 89.7781 22.8233 90.3483 22.8233H93.2807V26H89.0043ZM97.1311 26V10.8495H100.919V12.56H101.469C101.693 11.9491 102.059 11.5011 102.568 11.216C103.098 10.9309 103.709 10.7884 104.401 10.7884H106.234V14.2095H104.34C103.362 14.2095 102.558 14.4742 101.927 15.0036C101.295 15.5127 100.98 16.3069 100.98 17.3862V26H97.1311ZM109.242 26V4.61818H113.091V26H109.242Z"
          fill="white"
        />
        <path
          fill-rule="evenodd"
          clip-rule="evenodd"
          d="M0 14C0 7.40034 0 4.1005 2.05025 2.05025C4.1005 0 7.40034 0 14 0H17C23.5997 0 26.8995 0 28.9497 2.05025C31 4.1005 31 7.40034 31 14V17C31 23.5997 31 26.8995 28.9497 28.9497C26.8995 31 23.5997 31 17 31H14C7.40034 31 4.1005 31 2.05025 28.9497C0 26.8995 0 23.5997 0 17V14ZM19.7306 22.7901C19.698 19.5339 18.3899 16.4203 16.0874 14.1177C13.7848 11.8151 10.6711 10.507 7.41494 10.4744C6.0343 10.4606 4.90381 11.569 4.89 12.9496C4.87634 14.3302 5.98432 15.4604 7.36488 15.4742C9.3124 15.4937 11.1746 16.276 12.5518 17.6532C13.929 19.0304 14.7113 20.8926 14.7308 22.8401C14.7446 24.2207 15.8749 25.3287 17.2554 25.315C18.636 25.3012 19.7444 24.1707 19.7306 22.7901ZM9.07881 21.1702C10.055 22.1465 10.0551 23.7295 9.07881 24.7057C8.10253 25.682 6.51959 25.682 5.54327 24.7057L5.52912 24.6916C4.55281 23.7153 4.55281 22.1324 5.52912 21.156C6.50543 20.1797 8.08834 20.1797 9.06465 21.156L9.07881 21.1702Z"
          fill="white"
        />
      </svg>
      <div
        style={{
          flex: 1,
          maxHeight: "20%",
        }}
      ></div>

      <h1 style={{ fontSize: "4rem" }}>Docs/{page.data.title}</h1>
      <p
        style={{
          fontSize: "1.4rem",
          marginTop: "-16px",
          fontFamily: "Geist",
          fontWeight: "400",
        }}
      >
        {page.data.description}
      </p>
    </div>,
    {
      width: 1200,
      height: 630,
      fonts: [
        {
          name: "Space Grotesk",
          data: await loadGoogleFont("Space+Grotesk:wght@700", `Docs/${page.data.title}`),
          style: "normal",
        },
        {
          name: "Geist",
          data: await loadGoogleFont("Geist:wght@400", page.data.description || ""),
          style: "normal",
        },
      ],
    },
  );
}

export function generateStaticParams() {
  return source.getPages().map((page) => ({
    lang: page.locale,
    slug: getPageImage(page).segments,
  }));
}
