# Runtara Frontend

## Table of Contents

- [Technologies Used](#technologies-used)
- [Application Structure](#application-structure)
- [Architecture Overview](#architecture-overview)
- [State Management](#state-management)
- [Routing](#routing)
- [Styling](#styling)
- [API Communication](#api-communication)
- [Testing](#testing)
- [Development Setup](#development-setup)
- [Build & Deployment](#build--deployment)
- [Contributing](#contributing)

## Technologies Used

### Core Framework & Language

- **React 18.3.1** - Modern React with hooks and concurrent features
- **TypeScript 5.5.3** - Type-safe JavaScript with strict typing
- **Vite 6.3.5** - Fast build tool and development server

### UI & Styling

- **Tailwind CSS 3.4.17** - Utility-first CSS framework with custom design system
- **Radix UI** - Accessible, headless component primitives
- **Lucide React** - Beautiful, customizable icons
- **Next Themes** - Dark/light mode support
- **Class Variance Authority** - Type-safe component variants

### State Management & Data Fetching

- **Zustand 4.5.5** - Lightweight state management
- **TanStack Query 5.55.4** - Server state management and data fetching
- **Immer 10.1.1** - Immutable state updates

### Routing & Navigation

- **React Router 7.5.3** - Declarative routing for React applications

### Forms & Validation

- **React Hook Form 7.53.0** - Performant forms with easy validation
- **Zod 3.23.8** - TypeScript-first schema validation

### Development & Build Tools

- **ESLint 9.9.0** - Code linting with TypeScript support
- **Prettier 3.3.3** - Code formatting
- **Vitest 3.1.3** - Fast unit testing framework
- **React Testing Library 16.3.0** - Simple and complete testing utilities

### API & Authentication

- **Axios 1.7.7** - HTTP client for API communication
- **OIDC Client TS 3.1.0** - OpenID Connect authentication
- **Swagger TypeScript API** - Auto-generated API clients

### Analytics & Monitoring

- **Plausible Analytics** (optional) - Privacy-focused pageview tracking, opt-in via `VITE_RUNTARA_PLAUSIBLE_DOMAIN`

## Application Structure

The project follows a **feature-based architecture** that promotes modularity and maintainability:

```
src/
├── features/                 # Feature-specific modules
│   ├── analytics/           # Analytics dashboard and reports
│   ├── connections/         # Connection management
│   ├── objects/            # Object schemas and instances
│   ├── scenarios/          # Scenario creation and management
│   └── triggers/           # Invocation triggers
├── shared/                 # Shared resources across features
│   ├── components/         # Reusable UI components
│   ├── config/            # Application configuration
│   ├── hooks/             # Custom React hooks
│   ├── layouts/           # Page layouts and structure
│   ├── pages/             # Shared pages (login, etc.)
│   ├── queries/           # API query definitions
│   ├── stores/            # Zustand stores
│   └── utils/             # Utility functions
├── lib/                   # Core utilities and helper functions
├── router/                # Application routing configuration
├── generated/             # Auto-generated API clients
├── assets/                # Static assets (images, icons)
└── test/                  # Test setup and utilities
```

### Feature Structure

Each feature follows a consistent internal structure:

- `components/` - Feature-specific React components
- `pages/` - Feature page components
- `hooks/` - Feature-specific custom hooks
- `types/` - TypeScript type definitions
- `utils/` - Feature utility functions

## Architecture Overview

### Design Pattern

The application implements a **modular, feature-based architecture** with the following principles:

- **Feature Isolation**: Each feature is self-contained with its own components, pages, and business logic
- **Shared Resources**: Common functionality is centralized in the `shared` directory
- **Separation of Concerns**: Clear separation between UI components, business logic, and data management
- **Scalability**: Architecture supports easy addition of new features without affecting existing ones

### Component Architecture

- **Atomic Design Principles**: Components are built from small, reusable primitives
- **Composition over Inheritance**: Flexible component composition using React patterns
- **Accessibility First**: Built on Radix UI primitives for accessibility compliance
- **TypeScript Integration**: Fully typed components with proper prop interfaces

## State Management

### Client State - Zustand

- **Lightweight**: Minimal boilerplate with excellent TypeScript support
- **Devtools Integration**: Development-time debugging and state inspection
- **Immer Integration**: Immutable state updates with mutable syntax
- **Modular Stores**: Separate stores for different concerns (auth, UI state, etc.)

Example store structure:

```typescript
interface AuthState {
  userGroups: string[];
  setUserGroups: (groups: string[]) => void;
  clearUserGroups: () => void;
}

export const useAuthStore = create<AuthState>()(
  devtools(
    immer((set) => ({
      userGroups: [],
      setUserGroups: (groups) => set({ userGroups: groups }),
      clearUserGroups: () => set({ userGroups: [] }),
    }))
  )
);
```

### Server State - TanStack Query

- **Caching**: Intelligent data caching with automatic invalidation
- **Background Updates**: Automatic refetching and background synchronization
- **Error Handling**: Comprehensive error handling and retry logic
- **Optimistic Updates**: UI updates before server confirmation

## Routing

### React Router v7

- **File-based Routing**: Organized route definitions in `src/router/index.tsx`
- **Protected Routes**: Authentication-based route protection with `PrivateRoute` component
- **Nested Routing**: Hierarchical route structure with layout components
- **Type Safety**: Full TypeScript support for route parameters and navigation

### Route Structure

- `/` - Dashboard (Scenarios overview)
- `/scenarios` - Scenario management
- `/invocation-triggers` - Trigger configuration
- `/connections` - Connection management
- `/objects` - Object schema and instance management
- `/analytics` - Analytics dashboard

## Styling

### Tailwind CSS

- **Utility-First**: Rapid UI development with utility classes
- **Custom Design System**: Extended color palette and design tokens
- **Responsive Design**: Mobile-first responsive utilities
- **Dark Mode**: Built-in dark/light theme support
- **Component Variants**: Type-safe component styling with CVA

### Design System

- **Typography**: Inter font family with system fallbacks
- **Color Palette**: Comprehensive color system with semantic naming
- **Spacing**: Consistent spacing scale using CSS custom properties
- **Animations**: Custom animations and transitions

## API Communication

### HTTP Client - Axios

- **Interceptors**: Request/response transformation and error handling
- **Authentication**: Automatic token injection for authenticated requests
- **Error Handling**: Centralized error handling with user-friendly messages

### Generated API Clients

- **Swagger Integration**: Auto-generated TypeScript clients from OpenAPI specs
- **Type Safety**: Full TypeScript coverage for API requests and responses
- **Multiple APIs**: Support for management and object model APIs

### API Scripts

```bash
# Generate from production API
npm run generate-api-management-prod
npm run generate-api-object-model-prod

# Generate from local development API
npm run generate-api-management-local
npm run generate-api-object-model-local
```

## Testing

### Testing Framework - Vitest

- **Fast Execution**: Lightning-fast test execution with ES modules
- **React Testing Library**: Component testing with user-centric approach
- **Coverage Reports**: Code coverage analysis and reporting
- **Watch Mode**: Development-friendly test watching

### Testing Strategy

- **Component Tests**: User interaction and rendering behavior
- **Hook Tests**: Custom hook functionality and state management
- **Store Tests**: Zustand store actions and state transformations
- **Utility Tests**: Pure function testing for utility modules

### Test Commands

```bash
npm test              # Run tests once
npm run test:watch    # Run tests in watch mode
npm run test:coverage # Run tests with coverage report
```

### Test Structure

- Tests are co-located with source files using `.test.ts` or `.test.tsx` extensions
- Test utilities and setup in `src/test/` directory
- Example tests available for components, hooks, and stores

## Development Setup

### Prerequisites

- **Node.js**: Version specified in `.node-version` file
- **npm**: Package manager (comes with Node.js)

### Getting Started

1. **Clone the repository**

   ```bash
   git clone <repository-url>
   cd runtara/crates/runtara-server/frontend
   ```

2. **Install dependencies**

   ```bash
   npm install
   ```

3. **Environment setup**

   ```bash
   cp .env.example .env
   ```

   Configure the following environment variables:

   - `VITE_RUNTARA_API_BASE_URL`: Base URL for the main API (default: http://localhost:8080)
   - `VITE_RUNTARA_PLAUSIBLE_DOMAIN` (optional): Plausible site name; leave blank to disable analytics
   - `VITE_RUNTARA_PLAUSIBLE_HOST` (optional): Plausible host, defaults to `https://plausible.io`
   - `VITE_OIDC_AUTHORITY`: OIDC authority URL
   - `VITE_OIDC_CLIENT_ID`: OIDC client ID
   - `VITE_OIDC_AUDIENCE`: OIDC audience URL

4. **Start development server**

   ```bash
   npm run dev
   ```

   The application will be available at `http://localhost:8081`

### Development Commands

```bash
npm run dev       # Start development server
npm run build     # Build for production
npm run preview   # Preview production build
npm run lint      # Run ESLint
npm test          # Run tests
```

## Build & Deployment

### Production Build

```bash
npm run build
```

The build process:

1. **TypeScript Compilation**: Type checking and compilation
2. **Vite Build**: Optimized production bundle creation
3. **Source Maps**: Generated for debugging and error tracking

### Build Output

- `dist/index.html` - Entry point
- `dist/assets/` - Optimized CSS and JavaScript bundles
- Source maps for production debugging

### Deployment Options

- **CloudFlare**: Optimized for Vite projects with zero configuration

### Environment Configuration

Different environment variables can be configured for different deployment environments:

- Development: `.env.local`
- Staging: Environment-specific variables
- Production: Secure environment variable injection

## Contributing

### Code Style Guidelines

- **Prettier**: Automatic code formatting (2 spaces, single quotes, semicolons)
- **ESLint**: TypeScript-integrated linting with strict rules
- **TypeScript**: Strict typing, avoid `any` type usage

### Development Workflow

1. Create feature branch from `main`
2. Implement changes with tests
3. Run linting and tests: `npm run lint && npm test`
4. Build verification: `npm run build`
5. Submit pull request with clear description

### Commit Standards

- Clear, descriptive commit messages
- Atomic commits for individual features/fixes
- Reference issue numbers when applicable

### Testing Requirements

- Write tests for new components and utilities
- Maintain or improve test coverage
- Follow existing test patterns and structure

For detailed development guidelines, see [`.junie/guidelines.md`](.junie/guidelines.md)
