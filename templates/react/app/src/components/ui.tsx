import type { ButtonHTMLAttributes, HTMLAttributes, PropsWithChildren } from 'react'

// Small shared primitives. Keep the monochrome theme; extend with shadcn/ui as needed.
export function Button({ className = '', ...props }: ButtonHTMLAttributes<HTMLButtonElement>) {
  return <button className={`button ${className}`} {...props} />
}

export function Card({ className = '', ...props }: HTMLAttributes<HTMLElement>) {
  return <section className={`card ${className}`} {...props} />
}

export function Badge({ children }: PropsWithChildren) {
  return <span className="badge">{children}</span>
}
