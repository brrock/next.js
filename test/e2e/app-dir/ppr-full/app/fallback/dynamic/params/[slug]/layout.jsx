export default function Layout({ children }) {
  return (
    <>
      <div data-layout={Math.random().toString(16).slice(2)} />
      {children}
    </>
  )
}

export const revalidate = 120

export async function generateStaticParams() {
  return [{ slug: 'static-01' }, { slug: 'static-02' }]
}
